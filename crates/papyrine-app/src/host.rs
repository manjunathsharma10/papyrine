//! The host-side half of the engine protocol (ARCHITECTURE sections 5 and 7).
//!
//! [`EngineHost`] owns the engine child and, per open document, the write-ahead
//! [`Journal`]. Every command goes through the same four steps:
//!
//! 1. `Intent` is appended to the journal (the engine has not seen the command yet),
//! 2. the engine applies it and answers with after-images,
//! 3. `Commit` with the after-images is appended, or the intent is closed as a clean failure,
//! 4. if the engine died in step 2, it is restarted, the document is reopened and the journal's
//!    committed records are replayed into it; the unfinished command is reported (and counts
//!    towards quarantine), never silently re-run.
//!
//! Saves use bytes the engine produced; this module does the atomic replace (temp file, fsync,
//! validation, rename) and then tells the engine, which rebases on the saved file.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use papyrine_cos::Secret;
use papyrine_engine::proto::{
    Committed, Delivery, DigestInfo, DocSummary, OpenParams, Query, QueryResult, RenderBase,
    ReplayCommit, SaveDecision, SaveKindDto, SaveMode, SnapshotUpdate,
};
use papyrine_engine::{Request, Response};
use papyrine_ipc::{
    AfterImage, BlobRef, CommandRequest, DocId, DocSource, ErrorCode, Handle, IpcError,
};
use papyrine_journal::{
    BaseInfo, Clock, Fs, Journal, JournalError, Options as JournalOptions, RealFs, Recovery,
    RecoveryRoot, Resolution, SystemClock, recover,
};
use papyrine_writer::{ReplaceOptions, Validation, atomic_replace};
use serde_json::Value;

use crate::spawn::{EngineChild, SpawnConfig, spawn_engine};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("engine: {0}")]
    Engine(#[from] IpcError),
    #[error("journal: {0}")]
    Journal(#[from] JournalError),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("save: {0}")]
    Save(#[from] papyrine_writer::Error),
    /// The engine died while handling a request. It has been restarted and the open documents
    /// restored from their journals; see the report for what did not finish.
    #[error("the engine crashed and was restarted")]
    EngineRestarted(Box<RestartReport>),
    /// The original file changed on disk since the journal was started; offer
    /// "Open recovered copy" instead of restoring over it.
    #[error("the original file changed since the unsaved changes were recorded")]
    OriginalChanged,
    #[error("the engine crashed {0} times in {1:?}; not restarting it again")]
    CrashLoop(usize, Duration),
    #[error("no such document")]
    NoDoc,
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = HostError> = std::result::Result<T, E>;

pub struct HostConfig {
    pub spawn: SpawnConfig,
    /// The per-user recovery directory (`recovery/<doc-id>/`).
    pub journal_root: PathBuf,
    pub fs: Arc<dyn Fs>,
    pub journal: JournalOptions,
    pub clock: Arc<dyn Clock>,
    /// Give up restarting after this many engine deaths inside the window.
    pub max_restarts: usize,
    pub restart_window: Duration,
}

impl HostConfig {
    pub fn new(spawn: SpawnConfig, journal_root: impl Into<PathBuf>) -> Self {
        HostConfig {
            spawn,
            journal_root: journal_root.into(),
            fs: Arc::new(RealFs),
            journal: JournalOptions::default(),
            clock: Arc::new(SystemClock),
            max_restarts: 5,
            restart_window: Duration::from_secs(60),
        }
    }
}

struct HostDoc {
    path: PathBuf,
    password: Option<String>,
    journal: Journal,
}

/// A command the engine never answered.
#[derive(Debug, Clone)]
pub struct Unfinished {
    pub doc: DocId,
    pub seq: u64,
    pub command: String,
    pub params: Value,
    /// How many times this exact command has now crashed the engine.
    pub crashes: u32,
    /// Quarantined for this document: it will not be run again.
    pub quarantined: bool,
}

#[derive(Debug)]
pub struct RestoredDoc {
    pub doc: DocId,
    pub summary: DocSummary,
    /// Snapshot updates produced while replaying, in order (apply after the render base).
    pub snapshots: Vec<SnapshotUpdate>,
    pub replayed: usize,
}

#[derive(Debug)]
pub struct RestartReport {
    pub docs: Vec<RestoredDoc>,
    pub unfinished: Vec<Unfinished>,
}

/// Result of recovering a document at launch.
#[derive(Debug)]
pub struct Recovered {
    pub doc: DocId,
    pub summary: DocSummary,
    pub snapshots: Vec<SnapshotUpdate>,
    pub replayed: usize,
    pub unfinished: Vec<Unfinished>,
}

#[derive(Debug)]
pub struct Executed {
    pub seq: u64,
    pub committed: Committed,
}

#[derive(Debug)]
pub struct Saved {
    pub kind: SaveKindDto,
    pub path: PathBuf,
    pub bytes_written: u64,
    pub unchanged: bool,
    pub suggest_optimize: Option<String>,
    pub history_reset: bool,
    pub notice: Option<String>,
    pub snapshot: SnapshotUpdate,
}

#[derive(Debug)]
pub enum SaveOutcome {
    Saved(Box<Saved>),
    Decision(SaveDecision),
}

pub struct EngineHost {
    cfg: HostConfig,
    root: RecoveryRoot,
    engine: Option<EngineChild>,
    docs: BTreeMap<DocId, HostDoc>,
    deaths: VecDeque<Instant>,
    next_doc: u64,
}

fn file_source(path: &Path) -> std::io::Result<DocSource> {
    let f = File::open(path)?;
    let len = f.metadata()?.len();
    Ok(DocSource::File {
        handle: Handle::from_file(f),
        len,
    })
}

fn journal_id(path: &Path) -> String {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let h = blake3::hash(canon.to_string_lossy().as_bytes()).to_hex();
    format!("doc-{}", &h.as_str()[..16])
}

fn to_journal_images(images: &[AfterImage]) -> Vec<papyrine_journal::AfterImage> {
    images
        .iter()
        .map(|i| papyrine_journal::AfterImage {
            id: papyrine_journal::ObjId {
                num: i.obj,
                generation: i.generation,
            },
            data: i.body.clone().unwrap_or_default(),
        })
        .collect()
}

fn from_journal_images(images: &[papyrine_journal::AfterImage]) -> Vec<AfterImage> {
    images
        .iter()
        .map(|i| AfterImage {
            obj: i.id.num,
            generation: i.id.generation,
            body: Some(i.data.clone()),
        })
        .collect()
}

fn replay_commits(rec: &Recovery) -> Vec<ReplayCommit> {
    rec.commits
        .iter()
        .map(|c| ReplayCommit {
            seq: c.seq,
            command: c.command.clone(),
            after_images: from_journal_images(&c.after_images),
        })
        .collect()
}

fn is_crash(e: &IpcError) -> bool {
    e.code == ErrorCode::ChildCrashed
}

/// Read the bytes of a delivery that is not shared memory.
pub fn read_delivery(d: &Delivery) -> std::io::Result<Vec<u8>> {
    match d {
        Delivery::Inline(v) => Ok(v.clone()),
        Delivery::File(f) => std::fs::read(&f.path),
        Delivery::Shared { .. } => Err(std::io::Error::other("shared delivery needs its region")),
    }
}

impl EngineHost {
    pub fn new(cfg: HostConfig) -> Result<Self> {
        let engine = spawn_engine(&cfg.spawn)?;
        let root = RecoveryRoot::new(cfg.fs.clone(), cfg.journal_root.clone());
        Ok(EngineHost {
            cfg,
            root,
            engine: Some(engine),
            docs: BTreeMap::new(),
            deaths: VecDeque::new(),
            next_doc: 1,
        })
    }

    pub fn recovery_root(&self) -> &RecoveryRoot {
        &self.root
    }

    pub fn engine_pid(&self) -> Option<u32> {
        self.engine.as_ref().map(|e| e.process.id())
    }

    /// `kill -9` the engine (tests, and the host's watchdog for a hung engine).
    pub fn kill_engine(&mut self) {
        if let Some(e) = self.engine.as_mut() {
            let _ = e.process.kill();
            let _ = e.process.wait();
        }
    }

    pub fn journal(&self, doc: DocId) -> Option<&Journal> {
        self.docs.get(&doc).map(|d| &d.journal)
    }

    pub fn path(&self, doc: DocId) -> Option<&Path> {
        self.docs.get(&doc).map(|d| d.path.as_path())
    }

    fn call(&mut self, req: Request) -> std::result::Result<Response, IpcError> {
        let Some(e) = self.engine.as_ref() else {
            return Err(IpcError::new(
                ErrorCode::ChildCrashed,
                "engine is not running",
            ));
        };
        e.client.call_blocking(req)
    }

    fn job(&mut self, req: Request) -> std::result::Result<Response, IpcError> {
        let Some(e) = self.engine.as_ref() else {
            return Err(IpcError::new(
                ErrorCode::ChildCrashed,
                "engine is not running",
            ));
        };
        e.client.start_job(req)?.wait()
    }

    fn doc(&self, doc: DocId) -> Result<&HostDoc> {
        self.docs.get(&doc).ok_or(HostError::NoDoc)
    }

    // ------------------------------------------------------------- open

    /// Open `path` with a fresh journal.
    pub fn open(&mut self, path: &Path, password: Option<&str>) -> Result<(DocId, DocSummary)> {
        let id = DocId(self.next_doc);
        self.next_doc += 1;
        let base = BaseInfo::from_file(path)?;
        let journal = Journal::create(
            &self.root,
            self.cfg.fs.clone(),
            &journal_id(path),
            &base,
            self.cfg.journal.clone(),
            self.cfg.clock.clone(),
        )?;
        let (summary, _, _) = self
            .open_in_engine(id, path, password, &journal, None)
            .inspect_err(|_| {
                let _ = journal.discard();
            })?;
        self.docs.insert(
            id,
            HostDoc {
                path: path.to_path_buf(),
                password: password.map(str::to_owned),
                journal,
            },
        );
        Ok((id, summary))
    }

    /// Reopen a document from its recovery directory (after the app was killed): the original
    /// plus the checkpoint and journal records. Refused when the original changed.
    pub fn recover(&mut self, path: &Path, password: Option<&str>) -> Result<Recovered> {
        let id = DocId(self.next_doc);
        self.next_doc += 1;
        let (journal, rec) = Journal::open(
            &self.root,
            self.cfg.fs.clone(),
            &journal_id(path),
            self.cfg.journal.clone(),
            self.cfg.clock.clone(),
        )?;
        if !rec.base.original_still_matches() {
            return Err(HostError::OriginalChanged);
        }
        let (summary, snapshots, replayed) =
            self.open_in_engine(id, path, password, &journal, Some(&rec))?;
        let mut unfinished = Vec::new();
        for u in &rec.unfinished {
            let crashes = journal.resolve_unfinished(u.seq, Resolution::Skip)?;
            unfinished.push(Unfinished {
                doc: id,
                seq: u.seq,
                command: u.command.clone(),
                params: u.params.clone(),
                crashes,
                quarantined: crashes >= papyrine_journal::CRASH_LIMIT,
            });
        }
        self.docs.insert(
            id,
            HostDoc {
                path: path.to_path_buf(),
                password: password.map(str::to_owned),
                journal,
            },
        );
        Ok(Recovered {
            doc: id,
            summary,
            snapshots,
            replayed,
            unfinished,
        })
    }

    /// Send `Open` and replay what the journal holds. Returns the summary, the snapshot updates
    /// replay produced, and the number of records applied.
    fn open_in_engine(
        &mut self,
        id: DocId,
        path: &Path,
        password: Option<&str>,
        journal: &Journal,
        rec: Option<&Recovery>,
    ) -> Result<(DocSummary, Vec<SnapshotUpdate>, usize)> {
        // A checkpoint is an ID-preserving full write of the state at its record; it replaces
        // the original as the base the remaining records apply to.
        let source = match rec
            .and_then(|r| r.checkpoint.as_ref())
            .filter(|c| c.blob_ok)
        {
            Some(c) => {
                let p = journal
                    .dir()
                    .join("blobs")
                    .join(papyrine_journal::hash_hex(&c.blob.hash));
                file_source(&p)?
            }
            None => file_source(path)?,
        };
        let summary = match self.call(Request::Open {
            doc: id,
            source,
            password: password.map(str::to_owned),
            params: OpenParams::default(),
        })? {
            Response::Opened(s) => *s,
            r => return Err(HostError::Other(format!("unexpected reply {r:?}"))),
        };
        let mut snapshots = Vec::new();
        let mut replayed = 0;
        if let Some(rec) = rec {
            let commits = replay_commits(rec);
            for chunk in commits.chunks(64) {
                let n = chunk.len();
                let chunk: Vec<ReplayCommit> = chunk
                    .iter()
                    .map(|c| ReplayCommit {
                        seq: c.seq,
                        command: c.command.clone(),
                        after_images: c.after_images.clone(),
                    })
                    .collect();
                match self.call(Request::Replay {
                    doc: id,
                    commits: chunk,
                    out_section: None,
                })? {
                    Response::Replayed(r) => snapshots.push(r.snapshot),
                    r => return Err(HostError::Other(format!("unexpected reply {r:?}"))),
                }
                replayed += n;
            }
        }
        Ok((summary, snapshots, replayed))
    }

    // --------------------------------------------------------- commands

    /// Run a command: Intent, engine, Commit.
    pub fn execute(
        &mut self,
        doc: DocId,
        name: &str,
        params: Value,
        blobs: Vec<Vec<u8>>,
    ) -> Result<Executed> {
        let journal = self.doc(doc)?.journal.clone();
        let mut jblobs = Vec::with_capacity(blobs.len());
        for b in &blobs {
            jblobs.push(journal.put_blob(b)?);
        }
        let token = journal.begin(name, params.clone(), jblobs)?;
        let seq = token.seq();
        let req = Request::Execute {
            doc,
            command: CommandRequest {
                name: name.to_owned(),
                params_json: params.to_string(),
            },
            blobs: blobs.into_iter().map(BlobRef::Inline).collect(),
            out_section: None,
        };
        let r = self.call(req);
        self.finish_step(&journal, token, seq, r)
    }

    pub fn undo(&mut self, doc: DocId) -> Result<Executed> {
        self.step(
            doc,
            "undo",
            Request::Undo {
                doc,
                out_section: None,
            },
        )
    }

    pub fn redo(&mut self, doc: DocId) -> Result<Executed> {
        self.step(
            doc,
            "redo",
            Request::Redo {
                doc,
                out_section: None,
            },
        )
    }

    fn step(&mut self, doc: DocId, name: &str, req: Request) -> Result<Executed> {
        let journal = self.doc(doc)?.journal.clone();
        let token = journal.begin(name, Value::Null, vec![])?;
        let seq = token.seq();
        let r = self.call(req);
        self.finish_step(&journal, token, seq, r)
    }

    fn finish_step(
        &mut self,
        journal: &Journal,
        token: papyrine_journal::IntentToken,
        seq: u64,
        r: std::result::Result<Response, IpcError>,
    ) -> Result<Executed> {
        match r {
            Ok(Response::Committed(c)) => {
                let created = c
                    .created
                    .iter()
                    .map(|&(num, generation)| papyrine_journal::ObjId { num, generation })
                    .collect();
                journal.commit(token, to_journal_images(&c.after_images), created)?;
                Ok(Executed { seq, committed: *c })
            }
            Ok(r) => {
                let _ = journal.fail(token);
                Err(HostError::Other(format!("unexpected reply {r:?}")))
            }
            Err(e) if is_crash(&e) => {
                // The intent stays open: restart_engine counts it as a crash and reports it.
                let report = self.restart_engine()?;
                Err(HostError::EngineRestarted(Box::new(report)))
            }
            Err(e) => {
                // A clean error: the engine rolled back, nothing was applied.
                let _ = journal.fail(token);
                Err(e.into())
            }
        }
    }

    // ---------------------------------------------------------- restart

    /// Start a new engine, reopen every document from its original (or checkpoint) and replay
    /// the journal. Commands that were in flight are resolved as crashed (quarantine counts
    /// them) and reported, never re-run.
    pub fn restart_engine(&mut self) -> Result<RestartReport> {
        let now = Instant::now();
        self.deaths.push_back(now);
        while self
            .deaths
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.cfg.restart_window)
        {
            self.deaths.pop_front();
        }
        if self.deaths.len() > self.cfg.max_restarts {
            return Err(HostError::CrashLoop(
                self.deaths.len(),
                self.cfg.restart_window,
            ));
        }
        if let Some(mut old) = self.engine.take() {
            let _ = old.process.kill();
            let _ = old.process.wait();
        }
        self.engine = Some(spawn_engine(&self.cfg.spawn)?);

        let ids: Vec<DocId> = self.docs.keys().copied().collect();
        let mut report = RestartReport {
            docs: Vec::new(),
            unfinished: Vec::new(),
        };
        for id in ids {
            let (journal, path, password) = {
                let d = &self.docs[&id];
                (d.journal.clone(), d.path.clone(), d.password.clone())
            };
            // What the log holds right now (read-only; the in-flight intent shows up here).
            let rec = recover(self.cfg.fs.as_ref(), journal.dir())?;
            for u in &rec.unfinished {
                let crashes = journal.resolve_unfinished(u.seq, Resolution::Skip)?;
                report.unfinished.push(Unfinished {
                    doc: id,
                    seq: u.seq,
                    command: u.command.clone(),
                    params: u.params.clone(),
                    crashes,
                    quarantined: crashes >= papyrine_journal::CRASH_LIMIT,
                });
            }
            let (summary, snapshots, replayed) =
                self.open_in_engine(id, &path, password.as_deref(), &journal, Some(&rec))?;
            report.docs.push(RestoredDoc {
                doc: id,
                summary,
                snapshots,
                replayed,
            });
        }
        Ok(report)
    }

    // ------------------------------------------------------- checkpoint

    /// Ask the engine for an ID-preserving full write and make it the journal's checkpoint.
    pub fn checkpoint(&mut self, doc: DocId) -> Result<()> {
        let journal = self.doc(doc)?.journal.clone();
        let f = match self.job(Request::Checkpoint { doc })? {
            Response::Checkpointed(f) => f,
            r => return Err(HostError::Other(format!("unexpected reply {r:?}"))),
        };
        let bytes = std::fs::read(&f.path)?;
        let _ = std::fs::remove_file(&f.path);
        journal.checkpoint(&bytes)?;
        Ok(())
    }

    /// Checkpoint when the journal passed 32 MB or 500 records.
    pub fn checkpoint_if_due(&mut self, doc: DocId) -> Result<bool> {
        if self.doc(doc)?.journal.checkpoint_due() {
            self.checkpoint(doc)?;
            return Ok(true);
        }
        Ok(false)
    }

    // ------------------------------------------------------------- save

    /// Save to the document's own path, or to `target` (Save As).
    pub fn save(
        &mut self,
        doc: DocId,
        mode: SaveMode,
        target: Option<&Path>,
    ) -> Result<SaveOutcome> {
        let (path, password) = {
            let d = self.doc(doc)?;
            (d.path.clone(), d.password.clone())
        };
        let target = target.unwrap_or(&path).to_path_buf();
        let payload = match self.job(Request::Save { doc, mode })? {
            Response::SaveReady(p) => *p,
            Response::SaveDecision(d) => return Ok(SaveOutcome::Decision(d)),
            r => return Err(HostError::Other(format!("unexpected reply {r:?}"))),
        };
        let secret = password.as_deref().map(Secret::from);
        let expect_pages = Some(payload.page_count as usize);
        let bytes_written;
        match payload.kind {
            SaveKindDto::Incremental => {
                let add = read_delivery(&payload.bytes)?;
                let base_len = payload.base_len;
                bytes_written = base_len + add.len() as u64;
                atomic_replace(
                    &target,
                    |tmp| {
                        // Copy the current file (cloned on APFS), then append the section.
                        std::fs::remove_file(tmp)?;
                        std::fs::copy(&path, tmp)?;
                        let mut f = File::options().append(true).open(tmp)?;
                        if f.metadata()?.len() != base_len {
                            return Err(papyrine_writer::Error::Chain(
                                "the file changed since the engine scanned it".into(),
                            ));
                        }
                        f.write_all(&add)?;
                        Ok(())
                    },
                    ReplaceOptions {
                        validation: Validation {
                            password: secret.clone(),
                            expect_pages,
                            prefix_of: Some((&path, base_len)),
                            allow_repairs: false,
                            renderer: None,
                        },
                        faults: None,
                    },
                )?;
            }
            SaveKindDto::Optimized => {
                let Delivery::File(f) = &payload.bytes else {
                    return Err(HostError::Other("optimized save without a file".into()));
                };
                bytes_written = f.len;
                let src = PathBuf::from(&f.path);
                atomic_replace(
                    &target,
                    |tmp| {
                        std::fs::remove_file(tmp)?;
                        std::fs::copy(&src, tmp)?;
                        Ok(())
                    },
                    ReplaceOptions {
                        validation: Validation {
                            password: secret.clone(),
                            expect_pages,
                            prefix_of: None,
                            allow_repairs: false,
                            renderer: None,
                        },
                        faults: None,
                    },
                )?;
                let _ = std::fs::remove_file(&src);
            }
        }
        let saved = match payload.kind {
            SaveKindDto::Optimized => Some(file_source(&target)?),
            SaveKindDto::Incremental => None,
        };
        let fin = match self.call(Request::SaveCommitted {
            doc,
            token: payload.token,
            saved,
        })? {
            Response::SaveFinished(f) => f,
            r => return Err(HostError::Other(format!("unexpected reply {r:?}"))),
        };
        // The saved file is the new base: the journal restarts from it.
        let base = BaseInfo::from_file(&target)?;
        {
            let d = self.docs.get_mut(&doc).ok_or(HostError::NoDoc)?;
            d.journal.rebase(&base)?;
            d.path = target.clone();
        }
        Ok(SaveOutcome::Saved(Box::new(Saved {
            kind: payload.kind,
            path: target,
            bytes_written,
            unchanged: payload.unchanged,
            suggest_optimize: payload.suggest_optimize,
            history_reset: fin.history_reset,
            notice: fin.notice,
            snapshot: fin.snapshot,
        })))
    }

    // ---------------------------------------------------------- queries

    pub fn query(&mut self, doc: DocId, query: Query) -> Result<QueryResult> {
        match self.call(Request::Query { doc, query })? {
            Response::Query(q) => Ok(*q),
            r => Err(HostError::Other(format!("unexpected reply {r:?}"))),
        }
    }

    pub fn digest(&mut self, doc: DocId, per_object: bool) -> Result<DigestInfo> {
        match self.call(Request::Digest { doc, per_object })? {
            Response::Digest(d) => Ok(d),
            r => Err(HostError::Other(format!("unexpected reply {r:?}"))),
        }
    }

    pub fn compact(
        &mut self,
        doc: DocId,
        level: papyrine_engine::proto::CompactLevel,
    ) -> Result<SnapshotUpdate> {
        match self.job(Request::Compact { doc, level })? {
            Response::Compacted(u) => Ok(u),
            r => Err(HostError::Other(format!("unexpected reply {r:?}"))),
        }
    }

    /// Close a document. `discard` deletes the journal (a clean close or "Don't save");
    /// otherwise it stays for recovery.
    pub fn close(&mut self, doc: DocId, discard: bool) -> Result<()> {
        let d = self.docs.remove(&doc).ok_or(HostError::NoDoc)?;
        let _ = self.call(Request::Close { doc });
        if discard {
            d.journal.discard()?;
        }
        Ok(())
    }

    /// Stop the engine cleanly.
    pub fn shutdown(mut self) {
        if let Some(e) = self.engine.take() {
            let _ = e.shutdown(Duration::from_secs(5));
        }
    }
}

/// The render base the renderer should open for a freshly opened document: the original file
/// the host already mapped, or the repaired full write the engine produced.
pub fn render_base_path(summary: &DocSummary, original: &Path) -> PathBuf {
    match &summary.render_base {
        RenderBase::Original => original.to_path_buf(),
        RenderBase::Repaired(f) => PathBuf::from(&f.path),
    }
}
