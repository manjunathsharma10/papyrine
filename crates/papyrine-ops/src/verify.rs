//! The shadow verifier (ARCHITECTURE 4.4 consistency guard): fingerprint every object before
//! and after a command and fail if anything changed that the change set does not account for.

use std::collections::{BTreeSet, HashMap};

use papyrine_cos::{Document, Fingerprint, ObjId};

use crate::changeset::ChangeSet;
use crate::error::{Mismatch, Result};
use crate::image::{ObjectImage, TRAILER, resolve};

pub struct Shadow {
    before: HashMap<ObjId, Fingerprint>,
}

impl Shadow {
    pub fn begin(doc: &Document) -> Result<Shadow> {
        let mut before = HashMap::new();
        for id in doc.object_ids()?.into_iter().chain([TRAILER]) {
            before.insert(id, doc.fingerprint(&resolve(doc, id)?)?);
        }
        Ok(Shadow { before })
    }

    /// (changed pre-existing objects, new objects)
    fn diff(&self, doc: &Document) -> Result<(Vec<ObjId>, Vec<ObjId>)> {
        let mut changed = Vec::new();
        let mut new = Vec::new();
        for id in doc.object_ids()?.into_iter().chain([TRAILER]) {
            let fp = doc.fingerprint(&resolve(doc, id)?)?;
            match self.before.get(&id) {
                None => new.push(id),
                Some(old) if *old != fp => changed.push(id),
                Some(_) => {}
            }
        }
        changed.sort();
        new.sort();
        Ok((changed, new))
    }

    /// Check a command's returned change set against what actually happened.
    pub fn check_apply(&self, doc: &Document, cs: &ChangeSet) -> Result<Mismatch> {
        let (changed, new) = self.diff(doc)?;
        let touched: BTreeSet<ObjId> = cs.before.iter().map(|i| i.id).collect();
        let created: BTreeSet<ObjId> = cs.created.iter().copied().collect();
        let mut m = Mismatch {
            unrecorded: changed
                .into_iter()
                .filter(|i| !touched.contains(i))
                .collect(),
            unlisted_created: new.into_iter().filter(|i| !created.contains(i)).collect(),
            stale_after: Vec::new(),
        };
        for img in &cs.after {
            if !img.matches(doc)? {
                m.stale_after.push(img.id);
            }
        }
        Ok(m)
    }

    /// Check an undo/redo: everything that changed must have been written through the context.
    ///
    /// With `allow_new`, objects that appeared are tolerated: rebuilding qpdf's page cache around
    /// a page-tree restore can make qpdf re-push inherited attributes into fresh indirect
    /// objects, which end up unreferenced once the recorded images are back.
    pub fn check_restore(
        &self,
        doc: &Document,
        touched: &BTreeSet<ObjId>,
        allow_new: bool,
    ) -> Result<Mismatch> {
        let (changed, new) = self.diff(doc)?;
        Ok(Mismatch {
            unrecorded: changed
                .into_iter()
                .filter(|i| !touched.contains(i))
                .collect(),
            unlisted_created: if allow_new { Vec::new() } else { new },
            stale_after: Vec::new(),
        })
    }
}

/// Ids among `images` that the live document no longer matches.
pub fn images_diverge(doc: &Document, images: &[ObjectImage]) -> Result<Vec<ObjId>> {
    let mut bad = Vec::new();
    for img in images {
        if !img.matches(doc)? {
            bad.push(img.id);
        }
    }
    Ok(bad)
}
