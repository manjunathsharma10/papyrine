use std::collections::{BTreeMap, BTreeSet};

use papyrine_cos::{Document, ObjId, Object};

use crate::changeset::{ChangeSet, RepaintHint};
use crate::error::{Error, Result};
use crate::image::{ObjectImage, TRAILER};
use crate::pagetree;

/// The only way a command may mutate a document.
///
/// Before changing an existing object a command calls [`touch`](Self::touch) (or one of the
/// mutation helpers, which do it for it); the first touch captures a before-image. Objects
/// created during the command are found afterwards because qpdf allocates ids above the previous
/// maximum. A command that mutates an object it did not touch is caught by the shadow verifier.
pub struct EditContext<'d> {
    doc: &'d Document,
    /// Highest object number in the document when the command started.
    base_max: u32,
    /// `None` image: touched while restoring (no before-image wanted).
    touched: BTreeMap<ObjId, Option<ObjectImage>>,
    page_tree: bool,
    affected: BTreeSet<usize>,
    structure: bool,
}

impl<'d> EditContext<'d> {
    pub fn new(doc: &'d Document) -> Result<Self> {
        let base_max = doc.object_ids()?.iter().map(|i| i.num).max().unwrap_or(0);
        Ok(EditContext {
            doc,
            base_max,
            touched: BTreeMap::new(),
            page_tree: false,
            affected: BTreeSet::new(),
            structure: false,
        })
    }

    pub fn doc(&self) -> &'d Document {
        self.doc
    }

    /// Record `obj` (which must be indirect) before it is modified.
    pub fn touch(&mut self, obj: &Object) -> Result<()> {
        match obj.id() {
            Some(id) => self.touch_id(id),
            None => Err(Error::NotIndirect(
                String::from_utf8_lossy(&obj.unparse().unwrap_or_default()).into_owned(),
            )),
        }
    }

    pub fn touch_id(&mut self, id: ObjId) -> Result<()> {
        // Objects created by this command have nothing to restore.
        if id != TRAILER && id.num > self.base_max {
            return Ok(());
        }
        if !self.touched.contains_key(&id) {
            let img = ObjectImage::capture(self.doc, id)?;
            self.touched.insert(id, Some(img));
        }
        Ok(())
    }

    pub fn touch_trailer(&mut self) -> Result<()> {
        self.touch_id(TRAILER)
    }

    /// Record everything a page insert/remove/move can change; see [`pagetree`].
    pub fn touch_page_tree(&mut self) -> Result<()> {
        pagetree::touch_page_tree(self)
    }

    pub(crate) fn mark_page_tree(&mut self) {
        self.page_tree = true;
    }

    /// `holder[key] = value`, recording `holder` first.
    pub fn set_key(&mut self, holder: &Object, key: &str, value: &Object) -> Result<()> {
        self.touch(holder)?;
        holder.dict_set(key, value)?;
        Ok(())
    }

    /// `holder.remove(key)`, recording `holder` first.
    pub fn remove_key(&mut self, holder: &Object, key: &str) -> Result<()> {
        self.touch(holder)?;
        holder.dict_remove(key)?;
        Ok(())
    }

    /// The pixels of page `index` (post-change numbering) changed.
    pub fn note_page(&mut self, index: usize) {
        self.affected.insert(index);
    }

    /// Page count or order changed.
    pub fn note_structure(&mut self) {
        self.structure = true;
    }

    pub fn touched_ids(&self) -> BTreeSet<ObjId> {
        self.touched.keys().copied().collect()
    }

    /// Build the change set for everything recorded so far. Commands return this from `apply`.
    pub fn changeset(&self) -> Result<ChangeSet> {
        let created: Vec<ObjId> = {
            let mut v: Vec<ObjId> = self
                .doc
                .object_ids()?
                .into_iter()
                .filter(|i| i.num > self.base_max)
                .collect();
            v.sort();
            v
        };
        let before: Vec<ObjectImage> = self.touched.values().flatten().cloned().collect();
        let touched: Vec<ObjId> = before.iter().map(|i| i.id).collect();
        let mut after = Vec::with_capacity(before.len() + created.len());
        for id in touched.iter().chain(&created) {
            after.push(ObjectImage::capture(self.doc, *id)?);
        }
        let affected_pages: Vec<usize> = self.affected.iter().copied().collect();
        let repaint = if self.structure {
            RepaintHint::All
        } else if affected_pages.is_empty() {
            RepaintHint::None
        } else {
            RepaintHint::Pages(affected_pages.clone())
        };
        Ok(ChangeSet {
            before,
            after,
            created,
            touched,
            affected_pages,
            page_tree: self.page_tree,
            repaint,
        })
    }

    /// Put `images` back. With `page_tree`, qpdf's page cache is rebuilt around the restore.
    pub fn restore_images(&mut self, images: &[ObjectImage], page_tree: bool) -> Result<()> {
        if page_tree {
            pagetree::invalidate_page_cache(self.doc)?;
            self.page_tree = true;
        }
        for img in images {
            self.touched.entry(img.id).or_insert(None);
            img.restore(self.doc)?;
        }
        Ok(())
    }

    /// Default `Command::revert`: restore every before-image.
    pub fn restore_before(&mut self, cs: &ChangeSet) -> Result<()> {
        self.restore_images(&cs.before, cs.page_tree)
    }

    /// Default `Command::reapply`: restore the after-images of pre-existing objects. Created
    /// objects survive undo as unreachable orphans, so they need no restore.
    pub fn restore_after(&mut self, cs: &ChangeSet) -> Result<()> {
        let created: BTreeSet<ObjId> = cs.created.iter().copied().collect();
        let imgs: Vec<ObjectImage> = cs
            .after
            .iter()
            .filter(|i| !created.contains(&i.id))
            .cloned()
            .collect();
        self.restore_images(&imgs, cs.page_tree)
    }

    /// Undo everything recorded so far (used when a command fails halfway).
    pub fn rollback(&mut self) -> Result<()> {
        let imgs: Vec<ObjectImage> = self.touched.values().flatten().cloned().collect();
        let pt = self.page_tree;
        self.restore_images(&imgs, pt)
    }
}
