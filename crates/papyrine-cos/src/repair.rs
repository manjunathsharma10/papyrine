use std::fmt;

use crate::ObjId;

/// One warning or repair qpdf performed while reading or writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairEntry {
    pub message: String,
    /// The object the message is about, when qpdf names one.
    pub object: Option<ObjId>,
    /// qpdf's raw locator ("object 4 0", "trailer", "xref table"), possibly empty.
    pub locator: String,
    /// Byte offset in the input, when known.
    pub offset: Option<u64>,
}

impl fmt::Display for RepairEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.locator.is_empty() {
            write!(f, "{}: ", self.locator)?;
        }
        f.write_str(&self.message)?;
        if let Some(o) = self.offset {
            write!(f, " (offset {o})")?;
        }
        Ok(())
    }
}

/// Everything qpdf had to repair or warn about for one document, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairLog {
    entries: Vec<RepairEntry>,
}

impl RepairLog {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, RepairEntry> {
        self.entries.iter()
    }

    pub fn entries(&self) -> &[RepairEntry] {
        &self.entries
    }

    /// True if any entry's message contains `needle` (case-insensitive).
    pub fn mentions(&self, needle: &str) -> bool {
        let n = needle.to_lowercase();
        self.entries
            .iter()
            .any(|e| e.message.to_lowercase().contains(&n))
    }

    pub(crate) fn extend_from_ffi(&mut self, raw: Vec<qpdf_sys::RepairEntry>) {
        for r in raw {
            self.entries.push(RepairEntry {
                object: (r.object_id > 0)
                    .then(|| ObjId::new(r.object_id as u32, r.object_gen.max(0) as u16)),
                offset: (r.offset > 0).then_some(r.offset as u64),
                message: r.message,
                locator: r.object,
            });
        }
    }
}

impl<'a> IntoIterator for &'a RepairLog {
    type Item = &'a RepairEntry;
    type IntoIter = std::slice::Iter<'a, RepairEntry>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
