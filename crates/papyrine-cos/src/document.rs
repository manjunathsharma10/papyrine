use std::cell::RefCell;
use std::fmt;
use std::path::Path;
use std::rc::Rc;

use cxx::UniquePtr;
use qpdf_sys as ffi;

use crate::encryption::EncryptionInfo;
use crate::object::Object;
use crate::repair::RepairLog;
use crate::{Error, Result, Secret};

/// Object number and generation of an indirect object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId {
    pub num: u32,
    pub generation: u16,
}

impl ObjId {
    pub fn new(num: u32, generation: u16) -> Self {
        ObjId { num, generation }
    }
}

impl fmt::Display for ObjId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// How to open a document.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub password: Option<Secret>,
    /// Reconstruct a broken xref table by scanning the file (default on).
    pub attempt_recovery: bool,
    pub ignore_xref_streams: bool,
    /// Stop after this many warnings (0 = qpdf default).
    pub max_warnings: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions {
            password: None,
            attempt_recovery: true,
            ignore_xref_streams: false,
            max_warnings: 0,
        }
    }
}

impl OpenOptions {
    pub fn with_password(password: impl Into<Secret>) -> Self {
        OpenOptions {
            password: Some(password.into()),
            ..Self::default()
        }
    }

    fn to_ffi(&self) -> ffi::OpenOptions {
        ffi::OpenOptions {
            password: self
                .password
                .as_ref()
                .map(|p| p.expose().to_vec())
                .unwrap_or_default(),
            attempt_recovery: self.attempt_recovery,
            ignore_xref_streams: self.ignore_xref_streams,
            max_warnings: self.max_warnings,
        }
    }
}

/// Cheap structural fingerprint of an object for journal and consistency checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    /// Serialized dictionary or scalar; indirect references stay as `n g R`.
    pub repr: Vec<u8>,
    pub is_stream: bool,
    pub stream_raw_len: u64,
    /// SHA-256 of the raw (encoded) stream bytes.
    pub stream_sha256: Option<[u8; 32]>,
}

pub(crate) struct Inner {
    // Field order matters: the qpdf instance is dropped before the memory it reads from.
    pub(crate) doc: UniquePtr<ffi::Doc>,
    log: RefCell<RepairLog>,
    // Documents whose objects were copied into this one; qpdf may still read their input.
    keepalive: RefCell<Vec<Rc<Inner>>>,
    _owner: Option<Box<dyn AsRef<[u8]>>>,
}

/// An open PDF. `!Send`: it must stay on the thread (document actor) that created it.
pub struct Document {
    pub(crate) inner: Rc<Inner>,
}

pub(crate) fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

impl Document {
    fn open_with(
        owner: Option<Box<dyn AsRef<[u8]>>>,
        f: impl FnOnce(&ffi::Doc) -> std::result::Result<(), cxx::Exception>,
    ) -> Result<Document> {
        let inner = Rc::new(Inner {
            doc: ffi::doc_new(),
            log: RefCell::new(RepairLog::default()),
            keepalive: RefCell::new(Vec::new()),
            _owner: owner,
        });
        match f(&inner.doc) {
            Ok(()) => Ok(Document { inner }),
            Err(e) => {
                let mut log = RepairLog::default();
                if let Ok(w) = ffi::doc_take_warnings(&inner.doc) {
                    log.extend_from_ffi(w);
                }
                Err(Error::from_exception(e).with_repairs(log))
            }
        }
    }

    /// Open a PDF held in memory without copying it. `data` can be a `Vec<u8>`, an `Arc<[u8]>`,
    /// a memory map, or anything else whose `as_ref()` slice stays put and unmodified for the
    /// life of the document (the contract of [`AsRef`] on owning buffers).
    pub fn open_bytes(data: impl AsRef<[u8]> + 'static, opts: &OpenOptions) -> Result<Document> {
        let owner: Box<dyn AsRef<[u8]>> = Box::new(data);
        let (ptr, len) = {
            let s: &[u8] = (*owner).as_ref();
            (s.as_ptr(), s.len())
        };
        let raw = opts.to_ffi();
        // SAFETY: `owner` is stored in the Inner next to the qpdf instance and dropped after it,
        // and its heap location does not move.
        Self::open_with(Some(owner), |d| unsafe {
            ffi::doc_open_slice(d, "memory buffer", ptr, len, &raw)
        })
    }

    /// Open a file through qpdf's own file input source.
    pub fn open_path(path: impl AsRef<Path>, opts: &OpenOptions) -> Result<Document> {
        let p = path_bytes(path.as_ref());
        let raw = opts.to_ffi();
        Self::open_with(None, |d| ffi::doc_open_file(d, &p, &raw))
    }

    /// A new, empty document (no pages).
    pub fn new_empty() -> Result<Document> {
        Self::open_with(None, ffi::doc_new_empty)
    }

    pub(crate) fn ffi(&self) -> &ffi::Doc {
        &self.inner.doc
    }

    pub(crate) fn wrap(&self, raw: UniquePtr<ffi::Obj>) -> Object {
        Object::new(self.inner.clone(), raw)
    }

    /// All warnings and repairs so far (open-time and later), in order.
    pub fn repair_log(&self) -> RepairLog {
        self.drain_warnings();
        self.inner.log.borrow().clone()
    }

    fn drain_warnings(&self) {
        if let Ok(w) = ffi::doc_take_warnings(&self.inner.doc) {
            self.inner.log.borrow_mut().extend_from_ffi(w);
        }
    }

    /// Return the accumulated log and start a new one.
    pub fn take_repair_log(&self) -> RepairLog {
        self.drain_warnings();
        std::mem::take(&mut *self.inner.log.borrow_mut())
    }

    /// Header version, e.g. `"1.7"`.
    pub fn pdf_version(&self) -> Result<String> {
        Ok(ffi::doc_version(self.ffi())?)
    }

    pub fn is_linearized(&self) -> Result<bool> {
        Ok(ffi::doc_is_linearized(self.ffi())?)
    }

    pub fn trailer(&self) -> Result<Object> {
        Ok(self.wrap(ffi::doc_trailer(self.ffi())?))
    }

    pub fn root(&self) -> Result<Object> {
        Ok(self.wrap(ffi::doc_root(self.ffi())?))
    }

    pub fn object_count(&self) -> Result<usize> {
        Ok(ffi::doc_object_count(self.ffi())?)
    }

    pub fn object_ids(&self) -> Result<Vec<ObjId>> {
        Ok(ffi::doc_all_objects(self.ffi())?
            .into_iter()
            .map(|p| ObjId::new(p.id as u32, p.gen_ as u16))
            .collect())
    }

    /// Resolve an indirect object. A missing object resolves to null, as in qpdf.
    pub fn object(&self, id: ObjId) -> Result<Object> {
        Ok(self.wrap(ffi::doc_get_object(
            self.ffi(),
            id.num as i32,
            i32::from(id.generation),
        )?))
    }

    /// `None` when the document is not encrypted.
    pub fn encryption(&self) -> Result<Option<EncryptionInfo>> {
        Ok(EncryptionInfo::from_raw(&ffi::doc_encryption_info(
            self.ffi(),
        )?))
    }

    /// The document-level file encryption key (None if not encrypted).
    pub fn encryption_key(&self) -> Result<Option<Secret>> {
        if self.encryption()?.is_none() {
            return Ok(None);
        }
        let buf = ffi::doc_encryption_key(self.ffi())?;
        Ok(Some(Secret::new(ffi::buf_data(&buf))))
    }

    pub fn page_count(&self) -> Result<usize> {
        Ok(ffi::doc_pages_count(self.ffi())?)
    }

    pub fn page(&self, index: usize) -> Result<Object> {
        Ok(self.wrap(ffi::doc_page(self.ffi(), index)?))
    }

    pub fn pages(&self) -> Result<Vec<Object>> {
        (0..self.page_count()?).map(|i| self.page(i)).collect()
    }

    pub fn find_page(&self, page: &Object) -> Result<usize> {
        Ok(ffi::doc_find_page(self.ffi(), page.raw())? as usize)
    }

    /// Append (or prepend when `first`) a page. A page from another document is copied across.
    pub fn add_page(&self, page: &Object, first: bool) -> Result<()> {
        self.note_foreign(page);
        ffi::doc_add_page(self.ffi(), page.raw(), first)?;
        Ok(())
    }

    pub fn add_page_at(&self, page: &Object, before: bool, reference: &Object) -> Result<()> {
        self.note_foreign(page);
        ffi::doc_add_page_at(self.ffi(), page.raw(), before, reference.raw())?;
        Ok(())
    }

    pub fn remove_page(&self, page: &Object) -> Result<()> {
        ffi::doc_remove_page(self.ffi(), page.raw())?;
        Ok(())
    }

    pub fn push_inherited_page_attributes(&self) -> Result<()> {
        ffi::doc_push_inherited(self.ffi())?;
        Ok(())
    }

    pub fn new_null(&self) -> Object {
        self.wrap(ffi::obj_new_null())
    }
    pub fn new_bool(&self, v: bool) -> Object {
        self.wrap(ffi::obj_new_bool(v))
    }
    pub fn new_int(&self, v: i64) -> Object {
        self.wrap(ffi::obj_new_int(v))
    }
    pub fn new_real(&self, v: f64) -> Result<Object> {
        Ok(self.wrap(ffi::obj_new_real(v)?))
    }
    /// `name` excludes the leading slash.
    pub fn new_name(&self, name: impl AsRef<[u8]>) -> Result<Object> {
        Ok(self.wrap(ffi::obj_new_name(&crate::object::slashed(name.as_ref()))?))
    }
    pub fn new_string(&self, s: impl AsRef<[u8]>) -> Result<Object> {
        Ok(self.wrap(ffi::obj_new_string(s.as_ref())?))
    }
    pub fn new_array(&self) -> Object {
        self.wrap(ffi::obj_new_array())
    }
    pub fn new_dict(&self) -> Object {
        self.wrap(ffi::obj_new_dict())
    }
    pub fn new_stream(&self, data: &[u8]) -> Result<Object> {
        Ok(self.wrap(ffi::doc_new_stream(self.ffi(), data)?))
    }
    /// Parse PDF object syntax, e.g. `<< /Type /Page >>`.
    pub fn parse_object(&self, text: impl AsRef<[u8]>) -> Result<Object> {
        Ok(self.wrap(ffi::obj_parse(text.as_ref())?))
    }

    /// Make a direct object indirect (assigns it a new object number).
    pub fn make_indirect(&self, obj: &Object) -> Result<Object> {
        Ok(self.wrap(ffi::doc_make_indirect(self.ffi(), obj.raw())?))
    }

    pub fn replace_object(&self, id: ObjId, obj: &Object) -> Result<()> {
        ffi::doc_replace_object(
            self.ffi(),
            id.num as i32,
            i32::from(id.generation),
            obj.raw(),
        )?;
        Ok(())
    }

    /// Copy an object (and everything it references) from another document into this one.
    pub fn copy_foreign(&self, foreign: &Object) -> Result<Object> {
        self.note_foreign(foreign);
        Ok(self.wrap(ffi::doc_copy_foreign(self.ffi(), foreign.raw())?))
    }

    fn note_foreign(&self, obj: &Object) {
        if !Rc::ptr_eq(&self.inner, &obj.doc) {
            self.inner.keepalive.borrow_mut().push(obj.doc.clone());
        }
    }

    /// Structural fingerprint; see [`Fingerprint`].
    pub fn fingerprint(&self, obj: &Object) -> Result<Fingerprint> {
        let f = ffi::obj_fingerprint(obj.raw())?;
        Ok(Fingerprint {
            repr: f.repr,
            is_stream: f.is_stream,
            stream_raw_len: f.stream_raw_len,
            stream_sha256: <[u8; 32]>::try_from(f.stream_sha256.as_slice()).ok(),
        })
    }
}
