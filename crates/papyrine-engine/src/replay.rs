//! Journal replay as an ordinary [`Command`].
//!
//! Replay applies recorded after-images; it never re-runs the original command. Running it
//! through `History::execute` means the replayed step gets a proper before-image set, so undo
//! history is rebuilt for free (the before-images are exactly what the document held before the
//! step).
//!
//! Objects the original command created must reappear under the same numbers, because later
//! records refer to them. A fresh qpdf instance allocates ids from its own maximum upwards, so
//! the command allocates placeholders until it reaches each wanted number. Gaps only occur when
//! the original session had unreachable orphans (a failed command's partial allocations); the
//! placeholders are unreachable nulls and are never written by a save.

use papyrine_cos::{Document, ObjId, ObjectKind};
use papyrine_ops::{
    ChangeSet, Command, EditContext, Error, LocalizedText, ObjectImage, Result, TRAILER,
};
use serde_json::{Value, json};

pub struct ReplayCommand {
    label: String,
    images: Vec<ObjectImage>,
    page_tree: bool,
}

impl ReplayCommand {
    pub const NAME: &'static str = "replay";

    pub fn new(label: impl Into<String>, images: Vec<ObjectImage>, page_tree: bool) -> Self {
        let mut images = images;
        images.sort_by_key(|i| i.id);
        ReplayCommand {
            label: label.into(),
            images,
            page_tree,
        }
    }
}

fn max_num(doc: &Document) -> Result<u32> {
    Ok(doc.object_ids()?.iter().map(|i| i.num).max().unwrap_or(0))
}

/// Allocate objects until `wanted` exists. `stream` says whether the object must be a stream
/// object (its raw bytes are installed by the later restore). `last` is the highest number
/// allocated so far; qpdf hands out the next number each time.
fn allocate(doc: &Document, wanted: ObjId, stream: bool, last: &mut u32) -> Result<()> {
    if wanted.generation != 0 {
        return Err(Error::invalid(format!(
            "cannot create object {wanted} with a non-zero generation"
        )));
    }
    let take = |obj: papyrine_cos::Object| -> Result<u32> {
        obj.id()
            .map(|i| i.num)
            .ok_or_else(|| Error::invalid("allocator returned a direct object"))
    };
    // Fillers: unreachable indirect nulls.
    while *last + 1 < wanted.num {
        *last = take(doc.make_indirect(&doc.new_null())?)?;
        if *last >= wanted.num {
            return Err(Error::invalid(format!(
                "object {wanted} is already taken (allocator is at {last})"
            )));
        }
    }
    let obj = if stream {
        doc.new_stream(b"")?
    } else {
        doc.make_indirect(&doc.new_null())?
    };
    *last = take(obj)?;
    if *last != wanted.num {
        return Err(Error::invalid(format!(
            "expected to allocate object {wanted}, the allocator gave {last}"
        )));
    }
    Ok(())
}

impl Command for ReplayCommand {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new(format!("cmd.{}", self.label.replace('_', "-")))
    }

    fn params(&self) -> Value {
        json!({ "replay": self.label })
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let max = max_num(doc)?;
        // Existing objects: capture before-images before anything is changed (the page-cache
        // rebuild below edits the page tree).
        for img in &self.images {
            if img.id == TRAILER || img.id.num <= max {
                cx.touch_id(img.id)?;
            }
        }
        // Created objects, ascending.
        let mut last = max;
        for img in self
            .images
            .iter()
            .filter(|i| i.id != TRAILER && i.id.num > max)
        {
            allocate(doc, img.id, img.stream.is_some(), &mut last)?;
        }
        // An existing object that should be a stream but is not (or the reverse) means the
        // document diverged from the one the journal was written against.
        for img in self
            .images
            .iter()
            .filter(|i| i.id != TRAILER && i.id.num <= max)
        {
            let is_stream = doc.object(img.id)?.kind()? == ObjectKind::Stream;
            if is_stream != img.stream.is_some() {
                return Err(Error::invalid(format!(
                    "object {} has a different kind than the journal recorded",
                    img.id
                )));
            }
        }
        cx.restore_images(&self.images, self.page_tree)?;
        cx.note_structure();
        cx.changeset()
    }
}
