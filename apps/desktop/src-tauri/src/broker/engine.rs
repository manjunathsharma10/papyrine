//! Engine side of the broker: opening (with journal replay), supervision, output
//! collection and snapshot updates. The engine speaks `papyrine_engine::proto`.

use std::path::PathBuf;
use std::sync::Arc;

use papyrine_engine::proto::{
    CompactLevel, Delivery, DocSummary, InfoDto, LabelRangeDto, OpenParams, OutputFile, Query,
    QueryResult, RenderBase, ReplayCommit, Request, Response, SnapshotAction, SnapshotUpdate,
};
use papyrine_ipc::{Client, DocId, IpcError, SharedRegion};
use papyrine_journal::{BaseInfo, Recovery, recover};

use super::Broker;
use crate::api::{DocumentMeta, FormKind, HostEvent};
use crate::error::{HostErr, Result, is_crash};
use crate::session::{LabelRange, Session};
use crate::util::{BaseFile, Section, dup_region, pdf_date_to_iso, within};

pub(crate) type EngineClient = Client<Request, Response>;

/// What the renderer must do after a snapshot update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderAction {
    None,
    /// Same base, new section list.
    Reopen,
    /// New base file.
    Rebase,
}

pub(crate) fn meta_from(info: &InfoDto, encrypted: bool, old: &DocumentMeta) -> DocumentMeta {
    DocumentMeta {
        title: info.title.clone().unwrap_or_default(),
        author: info.author.clone().unwrap_or_default(),
        subject: info.subject.clone().unwrap_or_default(),
        keywords: info.keywords.clone().unwrap_or_default(),
        producer: info.producer.clone().unwrap_or_default(),
        pdf_version: if info.version.is_empty() {
            old.pdf_version.clone()
        } else {
            info.version.clone()
        },
        encrypted,
        creator: info.creator.clone().unwrap_or_default(),
        created: info
            .creation_date
            .as_deref()
            .map(pdf_date_to_iso)
            .unwrap_or_default(),
        modified: info
            .mod_date
            .as_deref()
            .map(pdf_date_to_iso)
            .unwrap_or_default(),
        file_size: 0,
    }
}

fn form_kind_of(f: &papyrine_engine::proto::FormSummary) -> FormKind {
    match f.xfa.as_str() {
        "dynamic" => FormKind::XfaDynamic,
        "static" => FormKind::XfaStatic,
        _ if f.has_acroform => FormKind::Acroform,
        _ => FormKind::None,
    }
}

pub(crate) fn labels_from(l: &[LabelRangeDto]) -> Vec<LabelRange> {
    l.iter()
        .map(|r| LabelRange {
            start: r.start_index,
            style: r.style.clone(),
            prefix: r.prefix.clone(),
            first: r.first_number,
        })
        .collect()
}

/// Journal data -> protocol after-image (first byte 1 = present, 0 = deleted).
pub(crate) fn from_journal_images(
    images: &[papyrine_journal::AfterImage],
) -> Vec<papyrine_ipc::AfterImage> {
    images
        .iter()
        .map(|a| papyrine_ipc::AfterImage {
            obj: a.id.num,
            generation: a.id.generation,
            body: match a.data.split_first() {
                Some((1, rest)) => Some(rest.to_vec()),
                _ => None,
            },
        })
        .collect()
}

impl Broker {
    // ----------------------------------------------------------- engine output

    fn engine_temp(&self) -> Result<PathBuf> {
        self.inner
            .engine
            .temp_dir()
            .ok_or_else(|| HostErr::internal("the engine is not running"))
    }

    /// Read (and delete) a file the engine left in its temp directory.
    pub(crate) fn take_output_bytes(&self, f: &OutputFile) -> Result<Vec<u8>> {
        let dir = self.engine_temp()?;
        let p = within(&dir, std::path::Path::new(&f.path)).ok_or_else(|| {
            HostErr::internal("the engine returned a file outside its temp directory")
        })?;
        let bytes = std::fs::read(&p)?;
        let _ = std::fs::remove_file(&p);
        if bytes.len() as u64 != f.len {
            return Err(HostErr::internal("engine output has the wrong length"));
        }
        Ok(bytes)
    }

    /// Move an engine output file to `dest` (copy if the move crosses devices).
    pub(crate) fn take_output_file(&self, f: &OutputFile, dest: &std::path::Path) -> Result<()> {
        let dir = self.engine_temp()?;
        let p = within(&dir, std::path::Path::new(&f.path)).ok_or_else(|| {
            HostErr::internal("the engine returned a file outside its temp directory")
        })?;
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d)?;
        }
        if std::fs::rename(&p, dest).is_err() {
            std::fs::copy(&p, dest)?;
            let _ = std::fs::remove_file(&p);
        }
        Ok(())
    }

    fn section_from(&self, d: Delivery, region: Option<SharedRegion>) -> Result<Section> {
        match d {
            Delivery::Shared { len } => {
                let region = region
                    .ok_or_else(|| HostErr::internal("engine used a region it was not given"))?;
                if len as usize > region.len() {
                    return Err(HostErr::internal("engine wrote past its section region"));
                }
                Ok(Section { region, len })
            }
            Delivery::Inline(v) => section_from_bytes(&v),
            Delivery::File(f) => section_from_bytes(&self.take_output_bytes(&f)?),
        }
    }

    fn render_base_from(&self, s: &Session, f: &OutputFile) -> Result<Arc<BaseFile>> {
        let path = self.render_base_path(s);
        self.take_output_file(f, &path)?;
        Ok(Arc::new(BaseFile::open(&path)?))
    }

    /// Apply an engine snapshot update to the session's render snapshot. The caller then
    /// brings the renderer along (see [`Broker::render_sync`]).
    pub(crate) fn apply_snapshot(
        &self,
        s: &Session,
        up: SnapshotUpdate,
        region: Option<SharedRegion>,
    ) -> Result<(RenderAction, Option<CompactLevel>)> {
        let advice = up.advice;
        let action = match up.action {
            SnapshotAction::Unchanged => RenderAction::None,
            SnapshotAction::Append(d) => {
                let sec = self.section_from(d, region)?;
                s.st().sections.push(sec);
                RenderAction::Reopen
            }
            SnapshotAction::ReplaceSections(d) => {
                let sec = self.section_from(d, region)?;
                s.st().sections = vec![sec];
                RenderAction::Reopen
            }
            SnapshotAction::NewBase(f) => {
                let nb = match f {
                    Some(f) => Some(self.render_base_from(s, &f)?),
                    None => None,
                };
                let mut st = s.st();
                st.render_base = nb;
                st.sections.clear();
                RenderAction::Rebase
            }
        };
        let mut st = s.st();
        st.snapshot_epoch = up.epoch;
        if action != RenderAction::None {
            st.epoch += 1;
        }
        Ok((action, advice))
    }

    // ------------------------------------------------------------------ opening

    /// The engine's source for this open: a journal checkpoint if there is one, else the
    /// document's file. Returns the journal recovery used for replay.
    fn engine_source(&self, s: &Session) -> Result<(Arc<BaseFile>, bool, Option<Recovery>)> {
        let has_journal = s.st().journal.is_some();
        let rec = if has_journal {
            self.inner
                .recovery
                .doc_dir(&s.doc_key)
                .ok()
                .and_then(|d| recover(self.inner.fs.as_ref(), &d).ok())
        } else {
            None
        };
        if let Some(c) = rec.as_ref().and_then(|r| r.checkpoint.as_ref())
            && c.blob_ok
        {
            let journal = s.st().journal.as_ref().map(|j| j.journal.clone());
            if let Some(j) = journal {
                let bytes = j.read_blob(&c.blob)?;
                let path = self
                    .inner
                    .cfg
                    .cache_dir
                    .join("ckpt")
                    .join(format!("{}.pdf", s.doc_key));
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&path, bytes)?;
                return Ok((Arc::new(BaseFile::open(&path)?), true, rec));
            }
        }
        Ok((s.st().base.clone(), false, rec))
    }

    /// Open the document in `client` and replay the journal's commits into it. Used for the
    /// first open, an engine restart and crash recovery alike. Caller holds `cmd_lock`.
    pub(crate) fn engine_restore(
        &self,
        s: &Session,
        client: &EngineClient,
    ) -> Result<Box<DocSummary>> {
        let (source, is_ckpt, rec) = self.engine_source(s)?;
        let (src, password) = {
            let st = s.st();
            (source.source()?, st.password.clone())
        };
        let doc = DocId(s.id);
        let summary = match client.call_blocking(Request::Open {
            doc,
            source: src,
            password,
            params: OpenParams {
                skip_pages: true,
                spill_budget_bytes: None,
            },
        })? {
            Response::Opened(sum) => sum,
            other => return Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        };
        // Render base: what the engine says, resolved to a file.
        let render_base = match &summary.render_base {
            RenderBase::Original => is_ckpt.then(|| source.clone()),
            RenderBase::Repaired(f) => Some(self.render_base_from(s, f)?),
        };
        {
            let mut st = s.st();
            st.render_base = render_base;
            st.sections.clear();
            st.snapshot_epoch = 0;
            st.epoch += 1;
            st.render_gen = 0;
            st.base_is_checkpoint = st.base_is_checkpoint || is_ckpt;
            st.repaired = summary.repaired;
            st.signed = summary.signatures.signed;
            st.form_kind = form_kind_of(&summary.form);
            st.can_undo = summary.can_undo;
            st.can_redo = summary.can_redo;
            st.labels = labels_from(&summary.labels);
            st.meta = meta_from(&summary.info, summary.security.encrypted, &st.meta);
        }
        let commits: Vec<ReplayCommit> = rec
            .as_ref()
            .map(|r| {
                r.commits
                    .iter()
                    .map(|c| ReplayCommit {
                        seq: c.seq,
                        command: c.command.clone(),
                        after_images: from_journal_images(&c.after_images),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let had_checkpoint = rec.as_ref().is_some_and(|r| r.checkpoint.is_some());
        if !commits.is_empty() {
            let region = SharedRegion::create(1 << 20)?;
            let out = Some(dup_region(&region)?);
            let r = match client.call_blocking(Request::Replay {
                doc,
                commits,
                out_section: out,
            })? {
                Response::Replayed(r) => r,
                other => return Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
            };
            self.apply_snapshot(s, r.snapshot, Some(region))?;
            let mut st = s.st();
            st.can_undo = r.can_undo;
            st.can_redo = r.can_redo;
            st.dirty = true;
            // Properties may have changed in the replayed commits.
            drop(st);
            if let Ok(Response::Query(q)) = client.call_blocking(Request::Query {
                doc,
                query: Query::Info,
            }) && let QueryResult::Info(i) = *q
            {
                let mut st = s.st();
                let enc = st.meta.encrypted;
                st.meta = meta_from(&i, enc, &st.meta);
            }
        } else if had_checkpoint {
            s.st().dirty = true;
        }
        self.invalidate_doc(s.id);
        Ok(summary)
    }

    /// The engine connection with this document open at the current generation. Starts
    /// or restarts the engine as needed. Caller holds `cmd_lock`.
    pub(crate) fn engine_client(&self, s: &Session) -> Result<EngineClient> {
        let c = self.inner.engine.conn()?;
        let have = s.st().engine_gen;
        if have != c.generation {
            let restarted = have != 0;
            self.engine_restore(s, &c.client)?;
            s.st().engine_gen = c.generation;
            if restarted {
                let skipped = s.st().skipped_action.take();
                self.emit(HostEvent::EngineRestarted {
                    doc_id: s.doc_id(),
                    skipped_action: skipped,
                });
                self.notice(
                    "info",
                    "engine-restarted",
                    "Engine restarted; no changes lost.",
                    Some(s),
                );
            }
        }
        Ok(c.client)
    }

    /// First open: also returns the engine's view of the document.
    pub(crate) fn engine_first_open(&self, s: &Session) -> Result<Box<DocSummary>> {
        let c = self.inner.engine.conn()?;
        let summary = self.engine_restore(s, &c.client)?;
        s.st().engine_gen = c.generation;
        Ok(summary)
    }

    /// Run `f` against the engine, retrying once when it died *between* requests
    /// (idempotent calls only; commands go through `apply_op`).
    pub(crate) fn with_engine<T>(
        &self,
        s: &Session,
        f: impl Fn(&EngineClient) -> std::result::Result<T, IpcError>,
    ) -> Result<T> {
        for attempt in 0..2 {
            let client = self.engine_client(s)?;
            match f(&client) {
                Err(e) if is_crash(&e) && attempt == 0 => continue,
                r => return r.map_err(Into::into),
            }
        }
        Err(HostErr::internal("engine kept crashing"))
    }

    pub(crate) fn engine_query(&self, s: &Session, q: Query) -> Result<QueryResult> {
        let doc = DocId(s.id);
        match self.with_engine(s, |c| {
            c.call_blocking(Request::Query {
                doc,
                query: q.clone(),
            })
        })? {
            Response::Query(r) => Ok(*r),
            other => Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        }
    }

    /// Re-read the document properties after something that may have changed them.
    pub(crate) fn refresh_meta(&self, s: &Session) {
        if let Ok(QueryResult::Info(i)) = self.engine_query(s, Query::Info) {
            let mut st = s.st();
            let enc = st.meta.encrypted;
            st.meta = meta_from(&i, enc, &st.meta);
        }
    }

    pub(crate) fn refresh_labels(&self, s: &Session) {
        if let Ok(QueryResult::Labels(l)) = self.engine_query(s, Query::Labels) {
            let mut st = s.st();
            st.labels = labels_from(&l);
        }
    }

    /// Run a snapshot compaction the engine advised (host is idle: runs under `cmd_lock`).
    pub(crate) fn spawn_compaction(&self, s: &Arc<Session>, level: CompactLevel) {
        let b = self.clone();
        let s = s.clone();
        let _ = std::thread::Builder::new()
            .name("papyrine-compact".into())
            .spawn(move || {
                let _g = s.lock_cmd();
                if s.st().closed {
                    return;
                }
                let doc = DocId(s.id);
                let r = b.with_engine(&s, |c| c.call_blocking(Request::Compact { doc, level }));
                let Ok(Response::Compacted(up)) = r else {
                    return;
                };
                let region = None;
                if let Ok((action, _)) = b.apply_snapshot(&s, up, region) {
                    let _ = b.render_sync(&s, action);
                }
            });
    }
}

fn section_from_bytes(v: &[u8]) -> Result<Section> {
    let region = SharedRegion::create(v.len().max(1))?;
    region.write_at(0, v);
    Ok(Section {
        region,
        len: v.len() as u64,
    })
}

/// `BaseInfo` for the document's current file (journal base).
pub(crate) fn base_info(path: &std::path::Path) -> std::io::Result<BaseInfo> {
    BaseInfo::from_file(path)
}
