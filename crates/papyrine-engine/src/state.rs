//! One open document: the qpdf `Document` inside a `Model`, its undo `History`, the render
//! snapshot chain and the save tracker. Owned by exactly one actor thread (`Document` is `!Send`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use papyrine_core::CancelToken;
use papyrine_cos::{Document, ObjId, ObjectStreams, OpenOptions, Secret, StreamMode, WriteOptions};
use papyrine_ipc::{BlobRef, ChangeSetSummary, CommandRequest, DocId, SharedRegion, SourceBytes};
use papyrine_model::Model;
use papyrine_ops::{
    ChangeSummary, Command, CommandRegistry, DirStore, History, LocalizedText, ObjectImage,
    RepaintHint, TRAILER,
};
use papyrine_writer::{
    ChainState, FullWriteOptions, SectionRequest, SuggestOptimize, has_signatures, suggestion,
    write_full, write_section, xref_was_repaired,
};

use crate::guard::{Guarded, PanicSlot};
use crate::images;
use crate::proto::*;
use crate::query;
use crate::replay::ReplayCommand;
use crate::util::{deliver, substitute_blobs, temp_path, write_temp};

/// L1 merge trigger: more sections than this.
pub const L1_SECTIONS: u32 = 16;
/// L2 trigger: section bytes above `max(L2_MIN_BYTES, base / 4)`.
pub const L2_MIN_BYTES: u64 = 16 << 20;
/// Default in-memory budget for undo images before they spill to the scratch directory.
const DEFAULT_HISTORY_BUDGET: u64 = 64 << 20;

/// Newtype so shared immutable bytes can back a qpdf document.
struct Bytes(SourceBytes);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        (*self.0).as_ref()
    }
}

fn slice(b: &SourceBytes) -> &[u8] {
    (**b).as_ref()
}

/// The renderer's view of the document: `base ‖ section_1 ‖ … ‖ section_k`.
struct RenderChain {
    base_chain: ChainState,
    base_len: u64,
    tip: ChainState,
    /// Every object changed since the base (what an L1 merge must contain).
    dirty: BTreeSet<ObjId>,
    sections: u32,
    section_bytes: u64,
    epoch: u64,
}

impl RenderChain {
    fn new(chain: ChainState) -> Self {
        RenderChain {
            base_len: chain.len,
            base_chain: chain.clone(),
            tip: chain,
            dirty: BTreeSet::new(),
            sections: 0,
            section_bytes: 0,
            epoch: 0,
        }
    }

    fn rebase(&mut self, chain: ChainState) {
        let epoch = self.epoch + 1;
        *self = RenderChain::new(chain);
        self.epoch = epoch;
    }

    fn l2_due(&self) -> bool {
        self.section_bytes > L2_MIN_BYTES.max(self.base_len / 4)
    }
}

/// What Save needs: the chain of the on-disk file and what changed since it was written.
struct SaveTracker {
    /// `None`: the original cannot be extended (repaired); saves are full rewrites.
    chain: Option<ChainState>,
    /// Object -> the commit sequence that last changed it.
    dirty: BTreeMap<ObjId, u64>,
    trailer_dirty: Option<u64>,
    original_len: u64,
}

struct PendingSave {
    token: u64,
    kind: SaveKindDto,
    /// Chain after the appended section (incremental).
    chain: Option<ChainState>,
    /// Commit sequence the section was built at.
    upto: u64,
}

enum Step {
    Execute,
    Undo,
    Redo,
}

pub struct DocState {
    id: DocId,
    model: Model,
    history: History,
    registry: Arc<CommandRegistry>,
    source: SourceBytes,
    password: Option<Secret>,
    repaired: bool,
    render: RenderChain,
    save: SaveTracker,
    scratch: PathBuf,
    history_dir: PathBuf,
    temp_files: Vec<PathBuf>,
    panic_slot: PanicSlot,
    poisoned: bool,
    pending: Option<PendingSave>,
    commit_seq: u64,
    next_token: u64,
    history_budget: usize,
}

fn open_document(source: &SourceBytes, password: &Option<Secret>) -> Result<Document> {
    let opts = OpenOptions {
        password: password.clone(),
        ..OpenOptions::default()
    };
    Ok(Document::open_bytes(Bytes(source.clone()), &opts)?)
}

/// Fingerprint of every object (stream bytes included), in id order, trailer last.
pub fn document_digest(doc: &Document, per_object: bool) -> Result<DigestInfo> {
    let mut ids = doc.object_ids()?;
    ids.sort();
    let mut all = blake3::Hasher::new();
    let mut each = Vec::new();
    let mut count = 0u32;
    for id in ids.into_iter().chain([TRAILER]) {
        let hash = object_hash(&ObjectImage::capture(doc, id)?);
        all.update(&id.num.to_le_bytes());
        all.update(&id.generation.to_le_bytes());
        all.update(&hash);
        if per_object {
            each.push((id.num, id.generation, hash));
        }
        count += 1;
    }
    Ok(DigestInfo {
        objects: count,
        digest: *all.finalize().as_bytes(),
        per_object: each,
    })
}

/// Remove a direct `/Length <n>` from a stream dictionary's serialization. qpdf adds `/Length`
/// when it writes a stream, so a stream created in memory (no `/Length` yet, or none at all when
/// empty) and the same stream read back from a snapshot differ only in that entry.
fn strip_length(repr: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    const KEY: &[u8] = b"/Length";
    let Some(at) = repr.windows(KEY.len()).position(|w| w == KEY) else {
        return repr.into();
    };
    let mut i = at + KEY.len();
    let ws = |b: u8| matches!(b, b' ' | b'\n' | b'\r' | b'\t');
    if !repr.get(i).is_some_and(|&b| ws(b)) {
        return repr.into(); // `/Length1` and friends
    }
    while repr.get(i).is_some_and(|&b| ws(b)) {
        i += 1;
    }
    let digits = i;
    while repr.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == digits || repr.get(i).is_some_and(|b| !ws(*b) && *b != b'>') {
        return repr.into();
    }
    // `/Length 12 0 R` is an indirect length: keep it.
    let mut j = i;
    while repr.get(j).is_some_and(|&b| ws(b)) {
        j += 1;
    }
    let g = j;
    while repr.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    if j > g {
        let mut k = j;
        while repr.get(k).is_some_and(|&b| ws(b)) {
            k += 1;
        }
        if repr.get(k) == Some(&b'R') {
            return repr.into();
        }
    }
    let start = if at > 0 && ws(repr[at - 1]) {
        at - 1
    } else {
        at
    };
    let mut out = repr[..start].to_vec();
    out.extend_from_slice(&repr[i..]);
    out.into()
}

fn object_hash(img: &ObjectImage) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    if img.stream.is_some() {
        h.update(&strip_length(&img.repr));
    } else {
        h.update(&img.repr);
    }
    match &img.stream {
        Some(s) => {
            h.update(&[1]);
            h.update(s);
        }
        None => {
            h.update(&[0]);
        }
    }
    *h.finalize().as_bytes()
}

fn new_history(dir: &Path, budget: usize) -> Result<History> {
    let store = DirStore::new(dir)?;
    Ok(History::with_store(budget, Box::new(store)))
}

fn ids_of(summary: &ChangeSummary, with_created: bool) -> BTreeSet<ObjId> {
    let mut ids: BTreeSet<ObjId> = summary.touched.iter().copied().collect();
    if with_created {
        ids.extend(summary.created.iter().copied());
    }
    ids
}

impl DocState {
    pub fn open(
        id: DocId,
        source: SourceBytes,
        password: Option<String>,
        registry: Arc<CommandRegistry>,
        params: &OpenParams,
        scratch: PathBuf,
    ) -> Result<(DocState, DocSummary)> {
        let t0 = Instant::now();
        let password = password.map(Secret::from);
        let doc = open_document(&source, &password)?;
        let log = doc.repair_log();
        let xref_repaired = xref_was_repaired(&log);
        let chain = ChainState::scan(slice(&source));
        let repaired = xref_repaired || chain.is_err();
        let history_dir = scratch.join(format!("history-{}-{}", std::process::id(), id.0));
        let budget = params
            .spill_budget_bytes
            .unwrap_or(DEFAULT_HISTORY_BUDGET)
            .min(usize::MAX as u64) as usize;
        let history = new_history(&history_dir, budget)?;
        let mut temp_files = Vec::new();
        let original_len = slice(&source).len() as u64;

        let (render_base, render_chain, save_chain) = if repaired {
            // qpdf's repaired object graph is the truth; give the renderer the same one.
            let path = temp_path(&scratch, "render-base");
            let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
            write_full(&doc, &mut w, &FullWriteOptions::default(), &|| false)?;
            w.flush()?;
            drop(w);
            let len = std::fs::metadata(&path)?.len();
            let chain = ChainState::scan(&File::open(&path)?)?;
            temp_files.push(path.clone());
            (
                RenderBase::Repaired(OutputFile {
                    path: path.to_string_lossy().into_owned(),
                    len,
                }),
                chain,
                None,
            )
        } else {
            let c = chain?;
            (RenderBase::Original, c.clone(), Some(c))
        };

        let st = DocState {
            id,
            model: Model::new(doc),
            history,
            registry,
            source,
            password,
            repaired,
            render: RenderChain::new(render_chain),
            save: SaveTracker {
                chain: save_chain,
                dirty: BTreeMap::new(),
                trailer_dirty: None,
                original_len,
            },
            scratch,
            history_dir,
            temp_files,
            panic_slot: Arc::default(),
            poisoned: false,
            pending: None,
            commit_seq: 0,
            next_token: 1,
            history_budget: budget,
        };
        let mut summary = st.summary(params.skip_pages)?;
        summary.render_base = render_base;
        summary.open_micros = t0.elapsed().as_micros() as u64;
        Ok((st, summary))
    }

    fn doc(&self) -> &Document {
        self.model.document()
    }

    fn check_usable(&self) -> Result<()> {
        if self.poisoned {
            return Err(EngineError::Internal(
                "the document state is uncertain after an internal error; reopen it".into(),
            ));
        }
        Ok(())
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn mark_poisoned(&mut self) {
        self.poisoned = true;
    }

    // ------------------------------------------------------------ queries

    pub fn summary(&self, skip_pages: bool) -> Result<DocSummary> {
        let m = &self.model;
        let n = m.page_count()?;
        let pages = query::pages(m, 0, n)?;
        let annotations = query::annot_summary(m, &pages)?;
        let log = self.doc().repair_log();
        Ok(DocSummary {
            doc: self.id,
            page_count: n as u32,
            pages: if skip_pages { Vec::new() } else { pages },
            outline: query::outline(m)?,
            labels: query::labels(m)?,
            form: query::form_summary(m)?,
            annotations,
            info: query::info(m)?,
            security: query::security(m)?,
            signatures: {
                // The model finds signature *fields*; the save policy looks at signature
                // dictionaries wherever they are. Report the stricter of the two.
                let mut sig = query::signatures(m)?;
                if !sig.signed && has_signatures(self.doc())? {
                    sig.signed = true;
                    sig.requires_incremental_save = true;
                }
                sig
            },
            repaired: self.repaired,
            repair_log: log.iter().take(200).map(ToString::to_string).collect(),
            render_base: RenderBase::Original,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            open_micros: 0,
        })
    }

    pub fn query(&self, q: &Query) -> Result<QueryResult> {
        let m = &self.model;
        Ok(match q {
            Query::Summary => QueryResult::Summary(Box::new(self.summary(false)?)),
            Query::Pages { from, count } => {
                QueryResult::Pages(query::pages(m, *from as usize, *count as usize)?)
            }
            Query::Outline => QueryResult::Outline(query::outline(m)?),
            Query::Labels => QueryResult::Labels(query::labels(m)?),
            Query::Fields => QueryResult::Fields(query::fields(m)?),
            Query::Annotations { page } => {
                if *page as usize >= m.page_count()? {
                    return Err(EngineError::Invalid(format!("page {page} out of range")));
                }
                QueryResult::Annotations(query::annotations(m, *page as usize)?)
            }
            Query::Info => QueryResult::Info(Box::new(query::info(m)?)),
            Query::Security => QueryResult::Security(query::security(m)?),
        })
    }

    /// BLAKE3 over every object's fingerprint, in id order (the trailer included).
    pub fn digest(&self, per_object: bool) -> Result<DigestInfo> {
        document_digest(self.doc(), per_object)
    }

    // ----------------------------------------------------------- commands

    pub fn execute(
        &mut self,
        command: &CommandRequest,
        blobs: &[BlobRef],
        region: Option<&SharedRegion>,
    ) -> Result<Committed> {
        self.check_usable()?;
        let t0 = Instant::now();
        let mut params: serde_json::Value = if command.params_json.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&command.params_json)
                .map_err(|e| EngineError::Invalid(format!("params: {e}")))?
        };
        substitute_blobs(&mut params, blobs)?;
        let cmd = self.registry.create(&command.name, &params)?;
        let label = cmd.describe();
        let guarded: Box<dyn Command> = Box::new(Guarded::new(cmd, self.panic_slot.clone()));
        *self.panic_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let res = catch_unwind(AssertUnwindSafe(|| {
            self.history.execute(self.model.document(), guarded)
        }));
        let summary = self.settle(res)?;
        self.after_step(summary, label, Step::Execute, region, t0)
    }

    /// Turn the outcome of a history call into a result, containing panics.
    fn settle<T>(&mut self, res: std::thread::Result<papyrine_ops::Result<T>>) -> Result<T> {
        match res {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                let slot = self
                    .panic_slot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                match slot {
                    // The guard rolled the command back; the document is intact.
                    Some(msg) => Err(EngineError::Internal(format!("command panicked: {msg}"))),
                    None => Err(e.into()),
                }
            }
            Err(p) => {
                self.poisoned = true;
                Err(EngineError::Internal(format!(
                    "panic outside a command: {}",
                    panic_text(&*p)
                )))
            }
        }
    }

    pub fn undo(&mut self, region: Option<&SharedRegion>) -> Result<Committed> {
        self.check_usable()?;
        let t0 = Instant::now();
        let label = self.history.undo_description();
        let res = catch_unwind(AssertUnwindSafe(|| {
            self.history.undo(self.model.document())
        }));
        let Some(summary) = self.settle(res)? else {
            return Err(EngineError::Invalid("nothing to undo".into()));
        };
        self.after_step(
            summary,
            label.unwrap_or_else(|| LocalizedText::new("cmd.undo")),
            Step::Undo,
            region,
            t0,
        )
    }

    pub fn redo(&mut self, region: Option<&SharedRegion>) -> Result<Committed> {
        self.check_usable()?;
        let t0 = Instant::now();
        let label = self.history.redo_description();
        let res = catch_unwind(AssertUnwindSafe(|| {
            self.history.redo(self.model.document())
        }));
        let Some(summary) = self.settle(res)? else {
            return Err(EngineError::Invalid("nothing to redo".into()));
        };
        self.after_step(
            summary,
            label.unwrap_or_else(|| LocalizedText::new("cmd.redo")),
            Step::Redo,
            region,
            t0,
        )
    }

    /// Apply journalled after-images. One snapshot update covers the whole batch.
    pub fn replay(
        &mut self,
        commits: &[ReplayCommit],
        region: Option<&SharedRegion>,
    ) -> Result<Replayed> {
        self.check_usable()?;
        let mut ids = BTreeSet::new();
        let mut pages = BTreeSet::new();
        let mut structure = false;
        for c in commits {
            let (imgs, tree) = images::unpack_all(&c.after_images)?;
            let cmd: Box<dyn Command> = Box::new(ReplayCommand::new(c.command.clone(), imgs, tree));
            let res = catch_unwind(AssertUnwindSafe(|| {
                self.history.execute(self.model.document(), cmd)
            }));
            let summary = self.settle(res).map_err(|e| {
                EngineError::Invalid(format!(
                    "replay of record {} ({}) failed: {e}",
                    c.seq, c.command
                ))
            })?;
            self.commit_seq += 1;
            self.track(&summary, &Step::Execute);
            self.invalidate_model(&summary);
            ids.extend(ids_of(&summary, true));
            pages.extend(summary.affected_pages.iter().map(|&p| p as u32));
            structure |= matches!(summary.repaint, RepaintHint::All);
        }
        let snapshot = self.snapshot_after(&ids, region)?;
        Ok(Replayed {
            applied: commits.len() as u32,
            snapshot,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            dirty_pages: pages.into_iter().collect(),
            structure_changed: structure,
        })
    }

    fn page_tree_ids(&self) -> BTreeSet<ObjId> {
        let mut s = BTreeSet::new();
        if let Ok(root) = self.doc().root() {
            match root.dict_get("Pages") {
                Ok(p) if p.is_indirect() => s.extend(p.id()),
                _ => s.extend(root.id()),
            }
        }
        s
    }

    fn invalidate_model(&self, summary: &ChangeSummary) {
        let ids: Vec<ObjId> = summary
            .touched
            .iter()
            .chain(&summary.created)
            .copied()
            .collect();
        if ids.contains(&TRAILER) || matches!(summary.repaint, RepaintHint::All) {
            self.model.invalidate_all();
        } else {
            self.model.invalidate(&ids);
        }
    }

    /// Record which objects a save must write.
    fn track(&mut self, summary: &ChangeSummary, step: &Step) {
        let seq = self.commit_seq;
        let put = |id: ObjId, save: &mut SaveTracker| {
            if id == TRAILER {
                save.trailer_dirty = Some(seq);
            } else {
                save.dirty.insert(id, seq);
            }
        };
        for &id in &summary.touched {
            put(id, &mut self.save);
        }
        match step {
            Step::Undo => {
                // Objects the undone command created are unreachable until a redo.
                for id in &summary.created {
                    self.save.dirty.remove(id);
                }
            }
            Step::Execute | Step::Redo => {
                for &id in &summary.created {
                    put(id, &mut self.save);
                }
            }
        }
    }

    fn after_step(
        &mut self,
        summary: ChangeSummary,
        label: LocalizedText,
        step: Step,
        region: Option<&SharedRegion>,
        t0: Instant,
    ) -> Result<Committed> {
        self.commit_seq += 1;
        self.track(&summary, &step);
        self.invalidate_model(&summary);

        let tree_ids = self.page_tree_ids();
        let page_tree = summary.touched.iter().any(|i| tree_ids.contains(i));
        let with_created = !matches!(step, Step::Undo);
        let ids = ids_of(&summary, with_created);
        let mut after_images = Vec::with_capacity(ids.len());
        for &id in &ids {
            after_images.push(images::to_ipc(
                &ObjectImage::capture(self.doc(), id)?,
                page_tree,
            ));
        }
        let snapshot = self.snapshot_after(&ids, region)?;
        let structure_changed = matches!(summary.repaint, RepaintHint::All);
        let cs = ChangeSetSummary {
            label: label.key.clone(),
            dirty_pages: summary.affected_pages.iter().map(|&p| p as u32).collect(),
            structure_changed,
            objects_changed: summary.touched.len() as u32,
            objects_added: if with_created {
                summary.created.len() as u32
            } else {
                0
            },
            objects_removed: 0,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
        };
        Ok(Committed {
            summary: cs,
            after_images,
            created: summary
                .created
                .iter()
                .map(|i| (i.num, i.generation))
                .collect(),
            snapshot,
            label_key: label.key,
            label_args: label.args.into_iter().collect(),
            engine_micros: t0.elapsed().as_micros() as u64,
        })
    }

    // ----------------------------------------------------------- snapshots

    fn unchanged(&self) -> SnapshotUpdate {
        SnapshotUpdate {
            action: SnapshotAction::Unchanged,
            epoch: self.render.epoch,
            sections: self.render.sections,
            section_bytes: self.render.section_bytes,
            advice: self.render.l2_due().then_some(CompactLevel::L2),
        }
    }

    /// Append the L0 section for `ids` (and merge to L1 past [`L1_SECTIONS`]).
    fn snapshot_after(
        &mut self,
        ids: &BTreeSet<ObjId>,
        region: Option<&SharedRegion>,
    ) -> Result<SnapshotUpdate> {
        let trailer = ids.contains(&TRAILER);
        let dirty: Vec<ObjId> = ids.iter().copied().filter(|i| *i != TRAILER).collect();
        if dirty.is_empty() && !trailer {
            return Ok(self.unchanged());
        }
        let doc = self.model.document();
        let section = write_section(
            doc,
            &self.render.tip,
            &SectionRequest {
                dirty: dirty.clone(),
                ..SectionRequest::default()
            },
        )?;
        self.render.dirty.extend(dirty);
        self.render.tip = section.chain.clone();
        self.render.sections += 1;
        self.render.section_bytes += section.bytes.len() as u64;
        self.render.epoch += 1;

        let action = if self.render.sections > L1_SECTIONS {
            let merged = self.merged_section()?;
            SnapshotAction::ReplaceSections(deliver(merged, region, &self.scratch, "section")?)
        } else {
            SnapshotAction::Append(deliver(section.bytes, region, &self.scratch, "section")?)
        };
        Ok(SnapshotUpdate {
            action,
            epoch: self.render.epoch,
            sections: self.render.sections,
            section_bytes: self.render.section_bytes,
            advice: self.render.l2_due().then_some(CompactLevel::L2),
        })
    }

    /// One section with the latest version of every object changed since the base.
    fn merged_section(&mut self) -> Result<Vec<u8>> {
        let section = write_section(
            self.model.document(),
            &self.render.base_chain,
            &SectionRequest {
                dirty: self.render.dirty.iter().copied().collect(),
                ..SectionRequest::default()
            },
        )?;
        self.render.tip = section.chain.clone();
        self.render.sections = 1;
        self.render.section_bytes = section.bytes.len() as u64;
        Ok(section.bytes)
    }

    pub fn compact(&mut self, level: CompactLevel, cancel: &CancelToken) -> Result<SnapshotUpdate> {
        self.check_usable()?;
        if self.pending.is_some() {
            // Compaction is skipped while a save is in progress (ADR-005).
            return Ok(self.unchanged());
        }
        match level {
            CompactLevel::L1 => {
                if self.render.sections <= 1 {
                    return Ok(self.unchanged());
                }
                let merged = self.merged_section()?;
                self.render.epoch += 1;
                Ok(SnapshotUpdate {
                    action: SnapshotAction::ReplaceSections(deliver(
                        merged,
                        None,
                        &self.scratch,
                        "section",
                    )?),
                    epoch: self.render.epoch,
                    sections: self.render.sections,
                    section_bytes: self.render.section_bytes,
                    advice: self.render.l2_due().then_some(CompactLevel::L2),
                })
            }
            CompactLevel::L2 => {
                let file = self.write_full_file("render-base", cancel)?;
                let chain = ChainState::scan(&File::open(&file.path)?)?;
                self.render.rebase(chain);
                Ok(SnapshotUpdate {
                    action: SnapshotAction::NewBase(Some(file)),
                    epoch: self.render.epoch,
                    sections: 0,
                    section_bytes: 0,
                    advice: None,
                })
            }
        }
    }

    /// ID-preserving full write into a scratch file.
    fn write_full_file(&mut self, stem: &str, cancel: &CancelToken) -> Result<OutputFile> {
        let path = temp_path(&self.scratch, stem);
        self.temp_files.push(path.clone());
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let c = cancel.clone();
        let r = write_full(
            self.model.document(),
            &mut w,
            &FullWriteOptions::default(),
            &move || c.is_cancelled(),
        );
        if let Err(e) = r {
            drop(w);
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
        w.flush()?;
        drop(w);
        Ok(OutputFile {
            len: std::fs::metadata(&path)?.len(),
            path: path.to_string_lossy().into_owned(),
        })
    }

    pub fn checkpoint(&mut self, cancel: &CancelToken) -> Result<OutputFile> {
        self.check_usable()?;
        self.write_full_file("checkpoint", cancel)
    }

    // ---------------------------------------------------------------- save

    fn trailer_size_fix(&self) -> Result<()> {
        // A trailer rebuilt from a damaged file can lack /Size, which qpdf's writer copies as
        // a gap into its output.
        let doc = self.doc();
        let t = doc.trailer()?;
        if !t.dict_has("Size")? {
            let max = doc.object_ids()?.iter().map(|i| i.num).max().unwrap_or(0);
            t.dict_set("Size", &doc.new_int(i64::from(max) + 1))?;
        }
        Ok(())
    }

    pub fn save(&mut self, mode: &SaveMode, cancel: &CancelToken) -> Result<SaveReply> {
        self.check_usable()?;
        let signed = has_signatures(self.doc())?;
        let (optimize, break_sigs) = match mode {
            SaveMode::Policy { break_signatures } => match (&self.save.chain, self.repaired) {
                (Some(_), false) => (false, *break_signatures),
                _ if signed && !break_signatures => {
                    return Ok(SaveReply::Decision(SaveDecision::SignedAndDamaged {
                        reason: if self.repaired {
                            "the file's cross-reference data had to be rebuilt".into()
                        } else {
                            "the end of the file cannot be extended".into()
                        },
                    }));
                }
                _ => (true, *break_signatures),
            },
            SaveMode::Optimized { break_signatures } => (true, *break_signatures),
        };
        if optimize && signed && !break_sigs {
            return Ok(SaveReply::Decision(SaveDecision::SignedRewrite));
        }
        let token = self.next_token;
        self.next_token += 1;
        let page_count = self.model.page_count()? as u32;
        let upto = self.commit_seq;

        if !optimize {
            let chain =
                self.save.chain.clone().ok_or_else(|| {
                    EngineError::Internal("incremental save without a chain".into())
                })?;
            let dirty: Vec<ObjId> = self.save.dirty.keys().copied().collect();
            if dirty.is_empty() && self.save.trailer_dirty.is_none() {
                self.pending = Some(PendingSave {
                    token,
                    kind: SaveKindDto::Incremental,
                    chain: Some(chain.clone()),
                    upto,
                });
                return Ok(SaveReply::Ready(SavePayload {
                    token,
                    kind: SaveKindDto::Incremental,
                    bytes: Delivery::Inline(Vec::new()),
                    base_len: chain.len,
                    total_len: chain.len,
                    sections: chain.sections,
                    page_count,
                    suggest_optimize: None,
                    history_will_reset: false,
                    unchanged: true,
                }));
            }
            let section = write_section(
                self.doc(),
                &chain,
                &SectionRequest {
                    dirty,
                    ..SectionRequest::default()
                },
            )?;
            let suggest = suggestion(
                &section.chain,
                section.chain.len,
                Some(self.save.original_len),
            )
            .map(describe_suggestion);
            self.pending = Some(PendingSave {
                token,
                kind: SaveKindDto::Incremental,
                chain: Some(section.chain.clone()),
                upto,
            });
            return Ok(SaveReply::Ready(SavePayload {
                token,
                kind: SaveKindDto::Incremental,
                bytes: deliver(section.bytes, None, &self.scratch, "save-section")?,
                base_len: chain.len,
                total_len: section.chain.len,
                sections: section.chain.sections,
                page_count,
                suggest_optimize: suggest,
                history_will_reset: false,
                unchanged: false,
            }));
        }

        // Optimized rewrite through qpdf's writer: object numbers change.
        self.trailer_size_fix()?;
        let path = temp_path(&self.scratch, "save-optimized");
        self.temp_files.push(path.clone());
        let opts = WriteOptions {
            object_streams: ObjectStreams::Generate,
            stream_data: StreamMode::Compress,
            ..WriteOptions::default()
        };
        let c = cancel.clone();
        let mut progress = move |_p: u8| !c.is_cancelled();
        let r = self
            .doc()
            .write_to_path_with_progress(&path, &opts, &mut progress);
        if let Err(e) = r {
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
        let len = std::fs::metadata(&path)?.len();
        let file = OutputFile {
            path: path.to_string_lossy().into_owned(),
            len,
        };
        self.pending = Some(PendingSave {
            token,
            kind: SaveKindDto::Optimized,
            chain: None,
            upto,
        });
        Ok(SaveReply::Ready(SavePayload {
            token,
            kind: SaveKindDto::Optimized,
            bytes: Delivery::File(file),
            base_len: 0,
            total_len: len,
            sections: 1,
            page_count,
            suggest_optimize: None,
            history_will_reset: true,
            unchanged: false,
        }))
    }

    /// The host replaced the file. `saved` is required after an optimized save.
    pub fn save_committed(
        &mut self,
        token: u64,
        saved: Option<SourceBytes>,
    ) -> Result<SaveFinished> {
        let Some(p) = self.pending.take_if(|p| p.token == token) else {
            return Err(EngineError::Invalid("unknown or stale save token".into()));
        };
        match p.kind {
            SaveKindDto::Incremental => {
                let upto = p.upto;
                self.save.dirty.retain(|_, seq| *seq > upto);
                if self.save.trailer_dirty.is_some_and(|s| s <= upto) {
                    self.save.trailer_dirty = None;
                }
                self.save.chain = p.chain;
                Ok(SaveFinished {
                    snapshot: self.unchanged(),
                    history_reset: false,
                    notice: None,
                })
            }
            SaveKindDto::Optimized => {
                let bytes = saved.ok_or_else(|| {
                    EngineError::Invalid("an optimized save needs the saved file".into())
                })?;
                self.reopen_from(bytes)?;
                Ok(SaveFinished {
                    snapshot: SnapshotUpdate {
                        action: SnapshotAction::NewBase(None),
                        epoch: self.render.epoch,
                        sections: 0,
                        section_bytes: 0,
                        advice: None,
                    },
                    history_reset: true,
                    notice: Some(
                        "Undo history before the optimized save is no longer available".into(),
                    ),
                })
            }
        }
    }

    /// Replace the open document with `bytes` (the file just saved). Object numbers changed, so
    /// the undo history cannot be carried over (`History` has no rebase-through-renumbering yet).
    fn reopen_from(&mut self, bytes: SourceBytes) -> Result<()> {
        let doc = open_document(&bytes, &self.password)?;
        let chain = ChainState::scan(slice(&bytes))?;
        self.history.clear();
        self.history = new_history(&self.history_dir, self.history_budget)?;
        self.model = Model::new(doc);
        self.repaired = false;
        self.save = SaveTracker {
            chain: Some(chain.clone()),
            dirty: BTreeMap::new(),
            trailer_dirty: None,
            original_len: slice(&bytes).len() as u64,
        };
        self.render.rebase(chain);
        self.source = bytes;
        Ok(())
    }

    pub fn write_scratch(&mut self, stem: &str, bytes: &[u8]) -> Result<OutputFile> {
        let f = write_temp(&self.scratch, stem, bytes)?;
        self.temp_files.push(PathBuf::from(&f.path));
        Ok(f)
    }
}

impl Drop for DocState {
    fn drop(&mut self) {
        for f in &self.temp_files {
            let _ = std::fs::remove_file(f);
        }
        let _ = std::fs::remove_dir_all(&self.history_dir);
    }
}

pub enum SaveReply {
    Ready(SavePayload),
    Decision(SaveDecision),
}

fn describe_suggestion(s: SuggestOptimize) -> String {
    match s {
        SuggestOptimize::AppendedBytes { appended, original } => {
            format!("{appended} bytes appended to an original of {original} bytes")
        }
        SuggestOptimize::Sections(n) => format!("{n} saved revisions"),
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".into()
    }
}

#[cfg(test)]
mod tests {
    use super::strip_length;

    #[test]
    fn length_is_stripped_only_when_direct() {
        assert_eq!(
            &*strip_length(b"<< /Length 3 /Filter /FlateDecode >>"),
            b"<< /Filter /FlateDecode >>"
        );
        assert_eq!(&*strip_length(b"<< /Length 12 >>"), b"<< >>");
        assert_eq!(
            &*strip_length(b"<< /Length 12 0 R >>"),
            b"<< /Length 12 0 R >>"
        );
        assert_eq!(&*strip_length(b"<< >>"), b"<< >>");
        assert_eq!(&*strip_length(b"<< /Length1 5 >>"), b"<< /Length1 5 >>");
    }
}
