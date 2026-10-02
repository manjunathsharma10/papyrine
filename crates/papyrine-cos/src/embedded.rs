use qpdf_sys as ffi;

use crate::{Document, ObjId, Result};

/// An entry of the `/EmbeddedFiles` name tree (file attachment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedFile {
    /// Name-tree key.
    pub name: String,
    pub filename: String,
    pub description: String,
    pub mime_type: Option<String>,
    /// `/Params /Size` when present.
    pub size: Option<u64>,
    pub created: Option<String>,
    pub modified: Option<String>,
    /// Raw MD5 checksum bytes when present.
    pub checksum: Option<Vec<u8>>,
    pub file_spec: ObjId,
    /// The embedded file stream (`document.object(id)?.stream_decoded(..)` yields the bytes).
    pub stream: Option<ObjId>,
}

impl Document {
    pub fn embedded_files(&self) -> Result<Vec<EmbeddedFile>> {
        let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
        let opt = |b: Vec<u8>| (!b.is_empty()).then(|| String::from_utf8_lossy(&b).into_owned());
        Ok(ffi::embedded_files_list(self.ffi())?
            .into_iter()
            .map(|e| EmbeddedFile {
                name: s(e.name),
                filename: s(e.filename),
                description: s(e.description),
                mime_type: opt(e.mime),
                size: u64::try_from(e.size).ok(),
                created: opt(e.created),
                modified: opt(e.modified),
                checksum: (!e.checksum.is_empty()).then_some(e.checksum),
                file_spec: ObjId::new(e.filespec_id.max(0) as u32, e.filespec_gen as u16),
                stream: (e.stream_id > 0)
                    .then(|| ObjId::new(e.stream_id as u32, e.stream_gen as u16)),
            })
            .collect())
    }
}
