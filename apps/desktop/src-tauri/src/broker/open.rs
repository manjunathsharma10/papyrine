//! Opening and closing documents.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use papyrine_engine::proto::Request as EngineRequest;
use papyrine_ipc::{DocId, RenderRequest};

use super::Broker;
use super::engine::RenderAction;
use papyrine_engine::proto::{OutlineDto, Query, QueryResult};

use crate::api::{DocumentInfo, DocumentMeta, FormKind, OutlineNode, PageInfo};
use crate::error::{Code, HostErr, Result};
use crate::session::{JournalHandle, Session, State};
use crate::util::{BaseFile, FileStat, file_name, random_hex};

/// What to open and how the journal relates to it.
pub(crate) struct OpenSpec {
    /// Shown to the user (`None` for dropped bytes and recovered copies).
    pub path: Option<PathBuf>,
    /// The file children open.
    pub base_path: PathBuf,
    pub name: String,
    pub owned_files: Vec<PathBuf>,
    pub password: Option<String>,
    /// An already-open journal (recovery) instead of a lazily created one.
    pub journal: Option<JournalHandle>,
    pub doc_key: Option<String>,
    pub base_is_checkpoint: bool,
}

impl Broker {
    /// Open a file from disk.
    pub fn open_path(&self, path: &Path, password: Option<String>) -> Result<DocumentInfo> {
        if !path.exists() {
            return Err(HostErr::not_found(format!(
                "No such file: {}",
                path.display()
            )));
        }
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let s = self.open_session(OpenSpec {
            name: file_name(&abs),
            path: Some(abs.clone()),
            base_path: abs.clone(),
            owned_files: vec![],
            password,
            journal: None,
            doc_key: None,
            base_is_checkpoint: false,
        })?;
        self.inner.recent.add(&abs);
        Ok(s.info())
    }

    /// Open bytes that arrived without a path (drag and drop in a plain webview). They
    /// are kept in `untitled/` so recovery can still find them after a crash.
    pub fn open_bytes(
        &self,
        name: &str,
        data: &[u8],
        password: Option<String>,
    ) -> Result<DocumentInfo> {
        let dir = self.inner.cfg.data_dir.join("untitled");
        std::fs::create_dir_all(&dir)?;
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || "._- ".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file = dir.join(format!("{}-{safe}", random_hex(4)));
        std::fs::write(&file, data)?;
        let s = self.open_session(OpenSpec {
            name: safe,
            path: None,
            base_path: file.clone(),
            owned_files: vec![file],
            password,
            journal: None,
            doc_key: None,
            base_is_checkpoint: false,
        });
        match s {
            Ok(s) => Ok(s.info()),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn open_session(&self, spec: OpenSpec) -> Result<Arc<Session>> {
        let cleanup_files = |files: &[PathBuf]| {
            for f in files {
                let _ = std::fs::remove_file(f);
            }
        };
        let base = match BaseFile::open(&spec.base_path) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                cleanup_files(&spec.owned_files);
                return Err(e);
            }
        };
        let id = self
            .inner
            .next_doc
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let doc_key = spec.doc_key.unwrap_or_else(|| random_hex(8));
        let stat = spec.path.as_deref().and_then(FileStat::of);
        let st = State {
            name: spec.name,
            path: spec.path,
            base_path: spec.base_path,
            meta: DocumentMeta {
                pdf_version: base.pdf_version(),
                ..DocumentMeta::default()
            },
            base,
            render_base: None,
            owned_files: spec.owned_files,
            sections: Vec::new(),
            snapshot_epoch: 0,
            labels: Vec::new(),
            engine_gen: 0,
            render_gen: 0,
            pages: Vec::new(),
            repaired: false,
            signed: false,
            form_kind: FormKind::None,
            base_is_checkpoint: spec.base_is_checkpoint,
            revision: 0,
            dirty: false,
            can_undo: false,
            can_redo: false,
            journal: spec.journal,
            password: spec.password,
            stat,
            external_flagged: false,
            unfinished: None,
            unfinished_params: None,
            checkpoint_disabled: false,
            skipped_action: None,
            journal_failed: false,
            epoch: 1,
            closed: false,
        };
        let s = Arc::new(Session::new(id, doc_key, st));

        // Renderer (first page) and engine (metadata, outline, repair log) open in parallel.
        let (eng, ren) = std::thread::scope(|sc| {
            let e = sc.spawn(|| {
                let _g = s.lock_cmd();
                self.engine_first_open(&s)
            });
            let r = self.render_open_info(&s);
            (
                e.join()
                    .unwrap_or_else(|_| Err(HostErr::internal("engine open panicked"))),
                r,
            )
        });
        let fail = |e: HostErr| -> HostErr {
            self.close_children(&s);
            let st = s.st();
            cleanup_files(&st.owned_files);
            e
        };
        let summary = match eng {
            Ok(i) => i,
            Err(e) => return Err(fail(e)),
        };
        let pages = match ren {
            Ok(p) => p,
            Err(e) => return Err(fail(e)),
        };
        {
            let mut st = s.st();
            st.pages = pages;
            st.apply_labels();
            if st.meta.title.is_empty() {
                st.meta.title = st
                    .name
                    .trim_end_matches(".pdf")
                    .trim_end_matches(".PDF")
                    .to_string();
            }
        }
        if summary.repaired {
            self.notice(
                "info",
                "repaired",
                "This file was damaged and has been repaired in memory.",
                Some(&s),
            );
        }
        // The engine may have switched the render base (repaired file, journal checkpoint)
        // or replayed journal commits (recovery) while the renderer opened the plain file.
        let rebase = {
            let st = s.st();
            st.render_gen == 0 || st.render_base.is_some() || !st.sections.is_empty()
        };
        if rebase && let Err(e) = self.render_sync(&s, RenderAction::Rebase) {
            return Err(fail(e));
        }
        self.inner
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, s.clone());
        self.watch_session(&s);
        Ok(s)
    }

    pub fn close_document(&self, doc_id: &str) -> Result<()> {
        let s = self.session(doc_id)?;
        let _g = s.lock_cmd();
        self.inner
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&s.id);
        self.inner.sched().purge_doc(s.id);
        self.invalidate_doc(s.id);
        self.unwatch_session(&s);
        self.close_children(&s);
        let mut st = s.st();
        st.closed = true;
        if let Some(j) = st.journal.take() {
            // "Don't save" and clean closes both end here: nothing to recover.
            let _ = j.journal.discard();
        }
        for f in st.owned_files.drain(..) {
            let _ = std::fs::remove_file(f);
        }
        st.render_base = None;
        let _ = std::fs::remove_file(self.render_base_path(&s));
        let _ = std::fs::remove_file(
            self.inner
                .cfg
                .cache_dir
                .join("ckpt")
                .join(format!("{}.pdf", s.doc_key)),
        );
        Ok(())
    }

    /// Tell running children to forget the document (never starts a child).
    pub(crate) fn close_children(&self, s: &Session) {
        let doc = DocId(s.id);
        if self.inner.engine.is_running()
            && let Ok(c) = self.inner.engine.conn()
        {
            let _ = c.client.call(EngineRequest::Close { doc });
        }
        if self.inner.renderer.is_running()
            && let Ok(c) = self.inner.renderer.conn()
        {
            let _ = c.client.call(RenderRequest::Close { doc });
        }
    }

    /// Current info of an open document (no side effects).
    pub fn session_info(&self, doc_id: &str) -> Result<DocumentInfo> {
        Ok(self.session(doc_id)?.info())
    }

    pub fn get_page_info(&self, doc_id: &str, page: usize) -> Result<PageInfo> {
        let s = self.session(doc_id)?;
        let st = s.st();
        st.pages
            .get(page)
            .cloned()
            .ok_or_else(|| HostErr::new(Code::NotFound, format!("no page {page}")))
    }

    /// Bookmarks, from the engine's model.
    pub fn get_outline(&self, doc_id: &str) -> Result<Vec<OutlineNode>> {
        let s = self.session(doc_id)?;
        let _g = s.lock_cmd();
        match self.engine_query(&s, Query::Outline)? {
            QueryResult::Outline(v) => Ok(v.iter().map(map_outline).collect()),
            other => Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        }
    }
}

fn map_outline(o: &OutlineDto) -> OutlineNode {
    OutlineNode {
        title: o.title.clone(),
        page: o.page.map(|p| p as usize),
        children: o.children.iter().map(map_outline).collect(),
    }
}
