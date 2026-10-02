use std::collections::HashMap;
use std::path::Path;

use cxx::UniquePtr;
use qpdf_sys as ffi;

use crate::document::path_bytes;
use crate::{Document, Error, ObjId, Result, Secret};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectStreams {
    /// Write every object at top level.
    Disable,
    /// Keep existing object streams.
    Preserve,
    /// Pack eligible objects into new object streams (smallest output).
    Generate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    Uncompress,
    Preserve,
    Compress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionRevision {
    /// 40-bit RC4. Weak; legacy compatibility only.
    R2,
    /// 128-bit RC4. Weak.
    R3,
    /// 128-bit RC4 (`aes: false`) or AES-128 (`aes: true`).
    R4 { aes: bool },
    /// Deprecated AES-256 extension level 3, for generating test files.
    R5,
    /// AES-256, the only password scheme in the PDF 2.0 specification.
    R6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintPermission {
    None,
    LowRes,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub accessibility: bool,
    pub extract: bool,
    pub assemble: bool,
    pub annotate_and_form: bool,
    pub form_filling: bool,
    pub modify_other: bool,
    pub print: PrintPermission,
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions {
            accessibility: true,
            extract: true,
            assemble: true,
            annotate_and_form: true,
            form_filling: true,
            modify_other: true,
            print: PrintPermission::Full,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EncryptionSpec {
    pub revision: EncryptionRevision,
    pub user_password: Secret,
    pub owner_password: Secret,
    pub permissions: Permissions,
    pub encrypt_metadata: bool,
}

impl EncryptionSpec {
    pub fn new(
        revision: EncryptionRevision,
        user_password: impl Into<Secret>,
        owner_password: impl Into<Secret>,
    ) -> Self {
        EncryptionSpec {
            revision,
            user_password: user_password.into(),
            owner_password: owner_password.into(),
            permissions: Permissions::default(),
            encrypt_metadata: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum EncryptionMode {
    /// Keep the source document's encryption (a no-op for unencrypted input).
    Preserve,
    /// Write without encryption.
    Decrypt,
    Encrypt(EncryptionSpec),
}

#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub object_streams: ObjectStreams,
    pub stream_data: StreamMode,
    pub recompress_flate: bool,
    pub linearize: bool,
    /// Fixed trailer /ID: byte-reproducible output for tests.
    pub static_id: bool,
    pub deterministic_id: bool,
    pub qdf: bool,
    pub normalize_content: bool,
    pub preserve_unreferenced: bool,
    pub newline_before_endstream: bool,
    pub encryption: EncryptionMode,
    pub min_version: Option<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            object_streams: ObjectStreams::Preserve,
            stream_data: StreamMode::Compress,
            recompress_flate: false,
            linearize: false,
            static_id: false,
            deterministic_id: false,
            qdf: false,
            normalize_content: false,
            preserve_unreferenced: false,
            newline_before_endstream: false,
            encryption: EncryptionMode::Preserve,
            min_version: None,
        }
    }
}

impl WriteOptions {
    pub(crate) fn to_ffi(&self) -> ffi::WriteOptions {
        let (preserve, enc) = match &self.encryption {
            EncryptionMode::Preserve => (true, None),
            EncryptionMode::Decrypt => (false, None),
            EncryptionMode::Encrypt(s) => (false, Some(s)),
        };
        let encryption = match enc {
            None => ffi::EncryptionParams {
                r: 0,
                aes: false,
                encrypt_metadata: true,
                user_password: vec![],
                owner_password: vec![],
                allow_accessibility: true,
                allow_extract: true,
                allow_assemble: true,
                allow_annotate_and_form: true,
                allow_form_filling: true,
                allow_modify_other: true,
                print: 2,
            },
            Some(s) => {
                let (r, aes) = match s.revision {
                    EncryptionRevision::R2 => (2, false),
                    EncryptionRevision::R3 => (3, false),
                    EncryptionRevision::R4 { aes } => (4, aes),
                    EncryptionRevision::R5 => (5, true),
                    EncryptionRevision::R6 => (6, true),
                };
                ffi::EncryptionParams {
                    r,
                    aes,
                    encrypt_metadata: s.encrypt_metadata,
                    user_password: s.user_password.expose().to_vec(),
                    owner_password: s.owner_password.expose().to_vec(),
                    allow_accessibility: s.permissions.accessibility,
                    allow_extract: s.permissions.extract,
                    allow_assemble: s.permissions.assemble,
                    allow_annotate_and_form: s.permissions.annotate_and_form,
                    allow_form_filling: s.permissions.form_filling,
                    allow_modify_other: s.permissions.modify_other,
                    print: match s.permissions.print {
                        PrintPermission::None => 0,
                        PrintPermission::LowRes => 1,
                        PrintPermission::Full => 2,
                    },
                }
            }
        };
        ffi::WriteOptions {
            object_streams: match self.object_streams {
                ObjectStreams::Disable => 0,
                ObjectStreams::Preserve => 1,
                ObjectStreams::Generate => 2,
            },
            stream_data: match self.stream_data {
                StreamMode::Uncompress => 0,
                StreamMode::Preserve => 1,
                StreamMode::Compress => 2,
            },
            recompress_flate: self.recompress_flate,
            linearize: self.linearize,
            static_id: self.static_id,
            deterministic_id: self.deterministic_id,
            qdf: self.qdf,
            normalize_content: self.normalize_content,
            preserve_unreferenced: self.preserve_unreferenced,
            newline_before_endstream: self.newline_before_endstream,
            preserve_encryption: preserve,
            min_version: self.min_version.clone().unwrap_or_default(),
            encryption,
        }
    }
}

/// Result of a write: the bytes (when written to memory) and the old-to-new object numbering.
pub struct WriteOutput {
    raw: UniquePtr<ffi::WriteOut>,
    renumber: HashMap<ObjId, ObjId>,
}

impl WriteOutput {
    /// The written file; empty when the output went to a path.
    pub fn bytes(&self) -> &[u8] {
        ffi::write_out_data(&self.raw)
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.bytes().to_vec()
    }

    /// `QPDFWriter::getRenumberedObjGen` for every object that existed before the write. Objects
    /// that were dropped (unreferenced) or folded into an object stream have no entry or map to
    /// their new number.
    pub fn renumbering(&self) -> &HashMap<ObjId, ObjId> {
        &self.renumber
    }
}

impl Document {
    pub fn write(&self, opts: &WriteOptions) -> Result<WriteOutput> {
        self.write_inner(opts, &[], ffi::ProgressBox::none())
    }

    pub fn write_to_path(&self, path: &Path, opts: &WriteOptions) -> Result<WriteOutput> {
        let p = non_empty_path(path)?;
        self.write_inner(opts, &p, ffi::ProgressBox::none())
    }

    /// Like [`Document::write`], reporting approximate progress (0..=100) to `progress`, which
    /// returns `false` to cancel; a cancelled write returns an error with [`Error::is_cancelled`] and the document
    /// stays usable. The callback runs on the calling thread during the write.
    pub fn write_with_progress(
        &self,
        opts: &WriteOptions,
        progress: &mut dyn FnMut(u8) -> bool,
    ) -> Result<WriteOutput> {
        let mut f = |p: i32| progress(p.clamp(0, 100) as u8);
        // SAFETY: the ProgressBox is consumed by this call and `f` outlives it.
        let pb = unsafe { ffi::ProgressBox::new(&mut f) };
        self.write_inner(opts, &[], pb)
    }

    /// File variant of [`Document::write_with_progress`]. A cancelled or failed write removes
    /// the partially written file.
    pub fn write_to_path_with_progress(
        &self,
        path: &Path,
        opts: &WriteOptions,
        progress: &mut dyn FnMut(u8) -> bool,
    ) -> Result<WriteOutput> {
        let p = non_empty_path(path)?;
        let mut f = |p: i32| progress(p.clamp(0, 100) as u8);
        // SAFETY: as in `write_with_progress`.
        let pb = unsafe { ffi::ProgressBox::new(&mut f) };
        let r = self.write_inner(opts, &p, pb);
        if r.as_ref().is_err_and(Error::is_cancelled) {
            let _ = std::fs::remove_file(path);
        }
        r
    }

    fn write_inner(
        &self,
        opts: &WriteOptions,
        path: &[u8],
        mut progress: ffi::ProgressBox,
    ) -> Result<WriteOutput> {
        let raw = ffi::doc_write(self.ffi(), &opts.to_ffi(), path, &mut progress)?;
        let map = ffi::write_out_renumber(&raw)?
            .into_iter()
            .map(|r| {
                (
                    ObjId::new(r.old_id as u32, r.old_gen as u16),
                    ObjId::new(r.new_id as u32, r.new_gen as u16),
                )
            })
            .collect();
        Ok(WriteOutput { raw, renumber: map })
    }
}

fn non_empty_path(path: &Path) -> Result<Vec<u8>> {
    let p = path_bytes(path);
    if p.is_empty() {
        return Err(Error::Range("empty output path".into()));
    }
    Ok(p)
}
