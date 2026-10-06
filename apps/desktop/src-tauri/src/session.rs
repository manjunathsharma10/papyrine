//! One open document as the broker sees it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use papyrine_journal::{Flusher, Journal};

use crate::api::{DocumentInfo, DocumentMeta, FormKind, PageInfo};
use crate::util::{BaseFile, FileStat, Section};

/// A command that was in flight when the previous process died, waiting for Redo or Skip.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unfinished {
    pub seq: u64,
    pub command: String,
    pub label: String,
    /// Crashes already counted against this exact command; 2 means quarantined.
    pub prior_crashes: u32,
    /// Journal blobs of the command's external inputs (for Redo).
    #[serde(skip)]
    pub blobs: Vec<papyrine_journal::BlobRef>,
}

pub struct JournalHandle {
    pub journal: Journal,
    /// Keeps the group-fsync thread alive.
    pub _flusher: Flusher,
}

/// One entry of the document's `/PageLabels` number tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelRange {
    pub start: u32,
    pub style: Option<String>,
    pub prefix: String,
    pub first: u32,
}

pub struct State {
    pub name: String,
    pub path: Option<PathBuf>,
    /// The file the journal is anchored to (the original; a temp file for dropped bytes).
    pub base_path: PathBuf,
    /// The file the engine opened: the source its incremental saves extend, and the render
    /// base unless `render_base` overrides it.
    pub base: Arc<BaseFile>,
    /// qpdf-repaired file, an engine L2 compaction or a journal checkpoint as render base.
    pub render_base: Option<Arc<BaseFile>>,
    /// Temp files this session owns (dropped-in bytes, recovered copies), removed on close.
    pub owned_files: Vec<PathBuf>,
    /// The renderer's snapshot sections after `render_base` (or `base`).
    pub sections: Vec<Section>,
    /// Engine snapshot epoch last applied (detects lost updates).
    pub snapshot_epoch: u64,
    pub engine_gen: u64,
    pub render_gen: u64,
    pub pages: Vec<PageInfo>,
    pub labels: Vec<LabelRange>,
    pub meta: DocumentMeta,
    pub repaired: bool,
    pub signed: bool,
    pub form_kind: FormKind,
    /// The engine opened a journal checkpoint (a different file than the user's), so Save
    /// cannot be an exact append to the user's original.
    pub base_is_checkpoint: bool,
    pub revision: u64,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub journal: Option<JournalHandle>,
    pub password: Option<String>,
    pub stat: Option<FileStat>,
    pub external_flagged: bool,
    pub unfinished: Option<Unfinished>,
    /// Params of the unfinished command, for Redo.
    pub unfinished_params: Option<serde_json::Value>,
    pub checkpoint_disabled: bool,
    /// The action that was running when the engine died (reported with `engine-restarted`).
    pub skipped_action: Option<String>,
    /// Creating the journal failed once; do not retry on every command.
    pub journal_failed: bool,
    /// Bumps whenever what the renderer shows changes; in-flight tiles from an older
    /// epoch are not cached.
    pub epoch: u64,
    pub closed: bool,
}

pub struct Session {
    pub id: u64,
    /// Journal directory name (`recovery/<doc_key>/`).
    pub doc_key: String,
    /// Serialises everything that mutates the document (execute, undo, redo, save).
    pub cmd_lock: Mutex<()>,
    /// Serialises (re)opening the document in the renderer.
    pub render_lock: Mutex<()>,
    st: Mutex<State>,
}

impl Session {
    pub fn new(id: u64, doc_key: String, st: State) -> Self {
        Self {
            id,
            doc_key,
            cmd_lock: Mutex::new(()),
            render_lock: Mutex::new(()),
            st: Mutex::new(st),
        }
    }

    pub fn st(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn doc_id(&self) -> String {
        format!("doc-{}", self.id)
    }

    pub fn lock_cmd(&self) -> MutexGuard<'_, ()> {
        self.cmd_lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn info(&self) -> DocumentInfo {
        self.st().info(self.id)
    }
}

impl State {
    pub fn info(&self, id: u64) -> DocumentInfo {
        let mut meta = self.meta.clone();
        meta.file_size = if self.path.is_some() {
            self.base.len
        } else {
            0
        };
        DocumentInfo {
            doc_id: format!("doc-{id}"),
            name: self.name.clone(),
            path: self.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            page_count: self.pages.len(),
            pages: self.pages.clone(),
            meta,
            repaired: self.repaired,
            revision: self.revision,
            dirty: self.dirty,
            can_undo: self.can_undo,
            can_redo: self.can_redo,
            signed: self.signed,
            form_kind: self.form_kind,
            form_scripts: false,
        }
    }

    pub fn render_source_base(&self) -> Arc<BaseFile> {
        self.render_base
            .clone()
            .unwrap_or_else(|| self.base.clone())
    }

    pub fn section_bytes(&self) -> u64 {
        self.sections.iter().map(|s| s.len).sum()
    }

    /// Fill `label` of every page from the label ranges.
    pub fn apply_labels(&mut self) {
        let ranges = self.labels.clone();
        for (i, p) in self.pages.iter_mut().enumerate() {
            p.label = page_label(&ranges, i as u32);
        }
    }
}

/// Parse `doc-<n>` back into the numeric id.
pub fn parse_doc_id(s: &str) -> Option<u64> {
    s.strip_prefix("doc-")?.parse().ok()
}

fn roman(mut n: u32, upper: bool) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if upper { s.to_uppercase() } else { s }
}

/// a..z, aa..zz, aaa.. (PDF 12.4.2: the letter repeats as the number grows).
fn letters(n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let idx = (n - 1) % 26;
    let reps = (n - 1) / 26 + 1;
    let c = (b'a' + idx as u8) as char;
    let c = if upper { c.to_ascii_uppercase() } else { c };
    std::iter::repeat_n(c, reps as usize).collect()
}

/// The label of 0-based page `index`, or `None` when the file defines no labels.
pub fn page_label(ranges: &[LabelRange], index: u32) -> Option<String> {
    let r = ranges.iter().rfind(|r| r.start <= index)?;
    let n = r.first.max(1) + (index - r.start);
    let num = match r.style.as_deref() {
        Some("D") => n.to_string(),
        Some("R") => roman(n, true),
        Some("r") => roman(n, false),
        Some("A") => letters(n, true),
        Some("a") => letters(n, false),
        _ => String::new(),
    };
    Some(format!("{}{num}", r.prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: u32, style: Option<&str>, prefix: &str, first: u32) -> LabelRange {
        LabelRange {
            start,
            style: style.map(String::from),
            prefix: prefix.into(),
            first,
        }
    }

    #[test]
    fn page_labels_follow_the_ranges() {
        let ranges = [
            r(0, Some("r"), "", 1),
            r(4, Some("D"), "", 1),
            r(10, Some("A"), "App-", 1),
        ];
        let l = |i| page_label(&ranges, i).unwrap();
        assert_eq!(l(0), "i");
        assert_eq!(l(3), "iv");
        assert_eq!(l(4), "1");
        assert_eq!(l(9), "6");
        assert_eq!(l(10), "App-A");
        assert_eq!(l(37), "App-BB");
        assert_eq!(page_label(&[], 3), None);
        assert_eq!(page_label(&[r(2, None, "Cover", 1)], 1), None);
        assert_eq!(page_label(&[r(0, None, "Cover", 1)], 5).unwrap(), "Cover");
        assert_eq!(roman(1994, true), "MCMXCIV");
    }
}
