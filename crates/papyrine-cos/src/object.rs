use std::rc::Rc;

use cxx::UniquePtr;
use qpdf_sys as ffi;

use crate::document::Inner;
use crate::{ObjId, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Null,
    Bool,
    Integer,
    Real,
    String,
    Name,
    Array,
    Dictionary,
    Stream,
    Operator,
    InlineImage,
    /// Reserved, unresolved or destroyed placeholders.
    Other(i32),
}

impl ObjectKind {
    fn from_code(c: i32) -> Self {
        match c {
            2 => ObjectKind::Null,
            3 => ObjectKind::Bool,
            4 => ObjectKind::Integer,
            5 => ObjectKind::Real,
            6 => ObjectKind::String,
            7 => ObjectKind::Name,
            8 => ObjectKind::Array,
            9 => ObjectKind::Dictionary,
            10 => ObjectKind::Stream,
            11 => ObjectKind::Operator,
            12 => ObjectKind::InlineImage,
            o => ObjectKind::Other(o),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeLevel {
    /// Flate, LZW, ASCII85, ASCIIHex, RunLength.
    Generalized,
    Specialized,
    All,
}

/// Stream bytes owned by C++, viewed without a copy.
pub struct StreamBytes(UniquePtr<ffi::Buf>);

impl StreamBytes {
    pub fn as_slice(&self) -> &[u8] {
        ffi::buf_data(&self.0)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

impl AsRef<[u8]> for StreamBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

/// Prefix a PDF name (given without its slash) with `/`, as qpdf expects.
pub(crate) fn slashed(name: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(name.len() + 1);
    v.push(b'/');
    v.extend_from_slice(name);
    v
}

/// A handle to a PDF object. Cloning is cheap and clones refer to the same underlying object.
/// Handles keep their document alive and are `!Send`.
pub struct Object {
    pub(crate) doc: Rc<Inner>,
    raw: UniquePtr<ffi::Obj>,
}

impl Clone for Object {
    fn clone(&self) -> Self {
        Object {
            doc: self.doc.clone(),
            raw: ffi::obj_clone(&self.raw),
        }
    }
}

impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.unparse() {
            Ok(b) => write!(f, "Object({})", String::from_utf8_lossy(&b)),
            Err(_) => f.write_str("Object(<unprintable>)"),
        }
    }
}

impl Object {
    pub(crate) fn new(doc: Rc<Inner>, raw: UniquePtr<ffi::Obj>) -> Self {
        Object { doc, raw }
    }

    pub(crate) fn raw(&self) -> &ffi::Obj {
        &self.raw
    }

    fn wrap(&self, raw: UniquePtr<ffi::Obj>) -> Object {
        Object {
            doc: self.doc.clone(),
            raw,
        }
    }

    pub fn kind(&self) -> Result<ObjectKind> {
        Ok(ObjectKind::from_code(ffi::obj_type(&self.raw)?))
    }

    pub fn is_indirect(&self) -> bool {
        ffi::obj_is_indirect(&self.raw)
    }

    /// Object id of an indirect object.
    pub fn id(&self) -> Option<ObjId> {
        let n = ffi::obj_id(&self.raw);
        (n > 0).then(|| ObjId::new(n as u32, ffi::obj_gen(&self.raw).max(0) as u16))
    }

    pub fn is_null(&self) -> Result<bool> {
        Ok(self.kind()? == ObjectKind::Null)
    }

    pub fn as_bool(&self) -> Result<bool> {
        Ok(ffi::obj_get_bool(&self.raw)?)
    }

    pub fn as_int(&self) -> Result<i64> {
        Ok(ffi::obj_get_int(&self.raw)?)
    }

    /// Integer or real as `f64`.
    pub fn as_f64(&self) -> Result<f64> {
        Ok(ffi::obj_get_numeric(&self.raw)?)
    }

    /// The textual form of a real, preserving how it was written.
    pub fn real_text(&self) -> Result<String> {
        Ok(ffi::obj_get_real_text(&self.raw)?)
    }

    /// Name bytes without the leading slash, with `#xx` escapes decoded.
    pub fn name(&self) -> Result<Vec<u8>> {
        let mut n = ffi::obj_get_name(&self.raw)?;
        if n.first() == Some(&b'/') {
            n.remove(0);
        }
        Ok(n)
    }

    /// Raw string bytes (PDFDocEncoding or UTF-16BE as stored; no transcoding).
    pub fn string(&self) -> Result<Vec<u8>> {
        Ok(ffi::obj_get_string(&self.raw)?)
    }

    pub fn array_len(&self) -> Result<usize> {
        Ok(ffi::array_len(&self.raw)? as usize)
    }

    pub fn array_get(&self, index: usize) -> Result<Object> {
        Ok(self.wrap(ffi::array_get(&self.raw, index as i32)?))
    }

    pub fn array_items(&self) -> Result<Vec<Object>> {
        (0..self.array_len()?).map(|i| self.array_get(i)).collect()
    }

    pub fn array_set(&self, index: usize, value: &Object) -> Result<()> {
        Ok(ffi::array_set(&self.raw, index as i32, &value.raw)?)
    }

    pub fn array_push(&self, value: &Object) -> Result<()> {
        Ok(ffi::array_append(&self.raw, &value.raw)?)
    }

    pub fn array_insert(&self, index: usize, value: &Object) -> Result<()> {
        Ok(ffi::array_insert(&self.raw, index as i32, &value.raw)?)
    }

    pub fn array_remove(&self, index: usize) -> Result<()> {
        Ok(ffi::array_erase(&self.raw, index as i32)?)
    }

    /// Dictionary keys without the leading slash. Works on streams (their dictionary).
    pub fn dict_keys(&self) -> Result<Vec<Vec<u8>>> {
        Ok(ffi::dict_keys(&self.raw)?
            .into_iter()
            .map(|mut b| {
                if b.v.first() == Some(&b'/') {
                    b.v.remove(0);
                }
                b.v
            })
            .collect())
    }

    pub fn dict_has(&self, key: impl AsRef<[u8]>) -> Result<bool> {
        Ok(ffi::dict_has(&self.raw, &slashed(key.as_ref()))?)
    }

    /// The value for `key`, or a null object when absent.
    pub fn dict_get(&self, key: impl AsRef<[u8]>) -> Result<Object> {
        Ok(self.wrap(ffi::dict_get(&self.raw, &slashed(key.as_ref()))?))
    }

    pub fn dict_set(&self, key: impl AsRef<[u8]>, value: &Object) -> Result<()> {
        Ok(ffi::dict_replace(
            &self.raw,
            &slashed(key.as_ref()),
            &value.raw,
        )?)
    }

    pub fn dict_remove(&self, key: impl AsRef<[u8]>) -> Result<()> {
        Ok(ffi::dict_remove(&self.raw, &slashed(key.as_ref()))?)
    }

    pub fn stream_dict(&self) -> Result<Object> {
        Ok(self.wrap(ffi::stream_dict(&self.raw)?))
    }

    /// The still-encoded (and, for encrypted files, decrypted) stream bytes.
    pub fn stream_raw(&self) -> Result<StreamBytes> {
        Ok(StreamBytes(ffi::stream_raw_data(&self.raw)?))
    }

    pub fn stream_decoded(&self, level: DecodeLevel) -> Result<StreamBytes> {
        let l = match level {
            DecodeLevel::Generalized => 1,
            DecodeLevel::Specialized => 2,
            DecodeLevel::All => 3,
        };
        Ok(StreamBytes(ffi::stream_data(&self.raw, l)?))
    }

    /// Replace the stream's data. `filter` and `decode_parms` describe how `data` is already
    /// encoded (pass `None` for plain data).
    pub fn stream_replace(
        &self,
        data: &[u8],
        filter: Option<&Object>,
        decode_parms: Option<&Object>,
    ) -> Result<()> {
        let null = ffi::obj_new_null();
        Ok(ffi::stream_replace_data(
            &self.raw,
            data,
            filter.map_or(&*null, |o| &o.raw),
            decode_parms.map_or(&*null, |o| &o.raw),
        )?)
    }

    /// PDF syntax. With `resolved`, indirect references are expanded (may be large).
    pub fn unparse_with(&self, resolved: bool) -> Result<Vec<u8>> {
        Ok(ffi::obj_unparse(&self.raw, resolved)?)
    }

    /// PDF syntax; an indirect object prints as `n g R`.
    pub fn unparse(&self) -> Result<Vec<u8>> {
        self.unparse_with(false)
    }
}
