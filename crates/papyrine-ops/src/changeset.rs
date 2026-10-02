use papyrine_cos::ObjId;

use crate::error::Result;
use crate::image::{ObjectImage, decode_images, encode_images};

/// What the UI should redraw after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepaintHint {
    None,
    /// Page indices (in the document after the change) whose pixels changed.
    Pages(Vec<usize>),
    /// Page count or order changed, or the change is too broad to enumerate.
    All,
}

/// Everything a command did, enough to undo and redo it without re-running it.
#[derive(Debug, Clone)]
pub struct ChangeSet {
    /// State of each pre-existing object at first touch (the trailer appears as object 0).
    /// Emptied while the images are spilled to disk.
    pub before: Vec<ObjectImage>,
    /// State of the same objects after the command, plus every created object.
    pub after: Vec<ObjectImage>,
    /// Objects allocated by the command (ids above the document's previous maximum).
    pub created: Vec<ObjId>,
    /// Every pre-existing object the command touched, sorted.
    pub touched: Vec<ObjId>,
    pub affected_pages: Vec<usize>,
    /// The page tree (`/Pages` nodes and `/Parent` links) may have changed, so qpdf's page
    /// cache must be rebuilt after restoring.
    pub page_tree: bool,
    pub repaint: RepaintHint,
}

/// A `ChangeSet` without its (large) images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSummary {
    pub created: Vec<ObjId>,
    pub touched: Vec<ObjId>,
    pub affected_pages: Vec<usize>,
    pub repaint: RepaintHint,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.touched.is_empty() && self.created.is_empty()
    }

    pub fn summary(&self) -> ChangeSummary {
        ChangeSummary {
            created: self.created.clone(),
            touched: self.touched.clone(),
            affected_pages: self.affected_pages.clone(),
            repaint: self.repaint.clone(),
        }
    }

    /// Bytes held by the images.
    pub fn image_bytes(&self) -> usize {
        self.before
            .iter()
            .chain(&self.after)
            .map(ObjectImage::size)
            .sum()
    }

    pub(crate) fn encode_images(&self) -> Vec<u8> {
        encode_images(&[&self.before, &self.after])
    }

    pub(crate) fn decode_images(&mut self, bytes: &[u8]) -> Result<()> {
        let mut g = decode_images(bytes, 2)?;
        self.after = g.pop().unwrap_or_default();
        self.before = g.pop().unwrap_or_default();
        Ok(())
    }

    pub(crate) fn drop_images(&mut self) {
        self.before = Vec::new();
        self.after = Vec::new();
    }
}
