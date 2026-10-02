use qpdf_sys as ffi;

use crate::{Document, ObjId, Result};

/// One bookmark, flattened in document order with its nesting depth.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub id: ObjId,
    /// 0 for top-level items.
    pub depth: usize,
    pub title: String,
    /// Destination page (zero-based) when it resolves to a page of this document.
    pub page: Option<usize>,
    /// `/Count`: negative for a collapsed item, positive for an open one.
    pub count: i32,
    pub uri: Option<String>,
    /// Named destination, when the target is given by name.
    pub dest_name: Option<String>,
}

impl OutlineItem {
    pub fn is_open(&self) -> bool {
        self.count > 0
    }
}

impl Document {
    /// The bookmark tree in pre-order (parents before children, siblings in order).
    pub fn outlines(&self) -> Result<Vec<OutlineItem>> {
        let non_empty =
            |b: Vec<u8>| (!b.is_empty()).then(|| String::from_utf8_lossy(&b).into_owned());
        Ok(ffi::outlines_read(self.ffi())?
            .into_iter()
            .map(|o| OutlineItem {
                id: ObjId::new(o.id as u32, o.gen_ as u16),
                depth: o.depth.max(0) as usize,
                title: String::from_utf8_lossy(&o.title).into_owned(),
                page: usize::try_from(o.page).ok(),
                count: o.count,
                uri: non_empty(o.uri),
                dest_name: non_empty(o.dest_name),
            })
            .collect())
    }
}
