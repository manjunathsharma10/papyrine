use papyrine_cos::Document;

use crate::changeset::{ChangeSet, ChangeSummary};
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::store::{MemStore, SpillStore};
use crate::text::LocalizedText;
use crate::verify::{Shadow, images_diverge};

struct Entry {
    cmd: Box<dyn Command>,
    cs: ChangeSet,
    /// Key in the spill store when the images are not resident.
    spilled: Option<u64>,
}

impl Entry {
    fn resident(&self) -> usize {
        if self.spilled.is_some() {
            0
        } else {
            self.cs.image_bytes()
        }
    }
}

/// Undo/redo stacks with a memory budget. Entries over the budget (oldest first) have their
/// images written to a [`SpillStore`] and are reloaded when undone or redone.
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    budget: usize,
    store: Box<dyn SpillStore>,
    next_key: u64,
    verify: bool,
}

fn spill_err(e: std::io::Error) -> Error {
    Error::Spill(e.to_string())
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// Unlimited budget; nothing is ever spilled.
    pub fn new() -> Self {
        Self::with_store(usize::MAX, Box::new(MemStore::default()))
    }

    /// Keep at most about `budget` bytes of images in memory, spilling the rest to `store`.
    pub fn with_store(budget: usize, store: Box<dyn SpillStore>) -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            budget,
            store,
            next_key: 0,
            verify: cfg!(debug_assertions),
        }
    }

    /// Run the shadow verifier around every command, undo and redo. On by default in debug
    /// builds (and therefore in tests), off in release.
    pub fn set_verify(&mut self, on: bool) {
        self.verify = on;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    pub fn undo_description(&self) -> Option<LocalizedText> {
        self.undo.last().map(|e| e.cmd.describe())
    }
    pub fn redo_description(&self) -> Option<LocalizedText> {
        self.redo.last().map(|e| e.cmd.describe())
    }
    /// Bytes of images currently in memory.
    pub fn resident_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .map(Entry::resident)
            .sum()
    }
    pub fn spilled_entries(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .filter(|e| e.spilled.is_some())
            .count()
    }

    /// Apply `cmd` as one undo step and clear the redo stack. If the command fails, everything
    /// it recorded is rolled back and nothing is pushed.
    pub fn execute(&mut self, doc: &Document, mut cmd: Box<dyn Command>) -> Result<ChangeSummary> {
        let shadow = if self.verify {
            Some(Shadow::begin(doc)?)
        } else {
            None
        };
        let mut cx = EditContext::new(doc)?;
        let cs = match cmd.apply(&mut cx) {
            Ok(cs) => cs,
            Err(e) => {
                let _ = cx.rollback();
                return Err(e);
            }
        };
        if let Some(sh) = shadow {
            let m = sh.check_apply(doc, &cs)?;
            if !m.is_empty() {
                let _ = cx.rollback();
                return Err(Error::Unrecorded {
                    command: cmd.name().to_owned(),
                    mismatch: m,
                });
            }
        }
        self.clear_redo();
        let summary = cs.summary();
        self.undo.push(Entry {
            cmd,
            cs,
            spilled: None,
        });
        self.enforce_budget()?;
        Ok(summary)
    }

    pub fn undo(&mut self, doc: &Document) -> Result<Option<ChangeSummary>> {
        self.step(doc, true)
    }

    pub fn redo(&mut self, doc: &Document) -> Result<Option<ChangeSummary>> {
        self.step(doc, false)
    }

    fn step(&mut self, doc: &Document, undo: bool) -> Result<Option<ChangeSummary>> {
        let Some(mut e) = (if undo {
            self.undo.pop()
        } else {
            self.redo.pop()
        }) else {
            return Ok(None);
        };
        let res = self.run_step(doc, &mut e, undo);
        let summary = e.cs.summary();
        // Moved to the other stack on success, put back where it came from on failure.
        if res.is_ok() == undo {
            self.redo.push(e);
        } else {
            self.undo.push(e);
        }
        res?;
        self.enforce_budget()?;
        Ok(Some(summary))
    }

    fn run_step(&mut self, doc: &Document, e: &mut Entry, undo: bool) -> Result<()> {
        self.load(e)?;
        let shadow = if self.verify {
            Some(Shadow::begin(doc)?)
        } else {
            None
        };
        let mut cx = EditContext::new(doc)?;
        if undo {
            e.cmd.revert(&mut cx, &e.cs)?;
        } else {
            e.cmd.reapply(&mut cx, &e.cs)?;
        }
        if let Some(sh) = shadow {
            let m = sh.check_restore(doc, &cx.touched_ids(), e.cs.page_tree)?;
            if !m.is_empty() {
                return Err(Error::Unrecorded {
                    command: e.cmd.name().to_owned(),
                    mismatch: m,
                });
            }
            let expect = if undo { &e.cs.before } else { &e.cs.after };
            let created: std::collections::BTreeSet<_> = e.cs.created.iter().copied().collect();
            let expect: Vec<_> = expect
                .iter()
                .filter(|i| !created.contains(&i.id))
                .cloned()
                .collect();
            let bad = images_diverge(doc, &expect)?;
            if !bad.is_empty() {
                return Err(Error::RestoreMismatch {
                    command: e.cmd.name().to_owned(),
                    ids: bad,
                });
            }
        }
        Ok(())
    }

    /// Bring a spilled entry's images back into memory.
    fn load(&mut self, e: &mut Entry) -> Result<()> {
        if let Some(key) = e.spilled {
            let bytes = self.store.get(key).map_err(spill_err)?;
            e.cs.decode_images(&bytes)?;
            self.store.remove(key).map_err(spill_err)?;
            e.spilled = None;
        }
        Ok(())
    }

    fn spill(&mut self, which_undo: bool, idx: usize) -> Result<()> {
        let key = self.next_key;
        let e = if which_undo {
            &mut self.undo[idx]
        } else {
            &mut self.redo[idx]
        };
        if e.spilled.is_some() {
            return Ok(());
        }
        let bytes = e.cs.encode_images();
        self.store.put(key, &bytes).map_err(spill_err)?;
        e.cs.drop_images();
        e.spilled = Some(key);
        self.next_key += 1;
        Ok(())
    }

    /// Spill the entries farthest from the cursor first: oldest undo, then deepest redo. The
    /// newest undo entry always stays resident.
    fn enforce_budget(&mut self) -> Result<()> {
        let mut total = self.resident_bytes();
        let keep = self.undo.len().saturating_sub(1);
        for i in 0..keep {
            if total <= self.budget {
                return Ok(());
            }
            let r = self.undo[i].resident();
            self.spill(true, i)?;
            total -= r;
        }
        for i in 0..self.redo.len() {
            if total <= self.budget {
                return Ok(());
            }
            let r = self.redo[i].resident();
            self.spill(false, i)?;
            total -= r;
        }
        Ok(())
    }

    fn clear_redo(&mut self) {
        for e in std::mem::take(&mut self.redo) {
            if let Some(k) = e.spilled {
                let _ = self.store.remove(k);
            }
        }
    }

    /// Forget all history (document closed or saved with history discarded).
    pub fn clear(&mut self) {
        self.clear_redo();
        for e in std::mem::take(&mut self.undo) {
            if let Some(k) = e.spilled {
                let _ = self.store.remove(k);
            }
        }
    }
}
