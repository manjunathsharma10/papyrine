//! Save and Save As: engine bytes -> validated atomic replace -> journal rebase
//! (ARCHITECTURE §4.5, §7). The engine decides incremental vs optimized (save policy);
//! the host writes the file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use papyrine_cos::Secret;
use papyrine_engine::proto::{Delivery, Request, Response, SaveDecision, SaveKindDto, SaveMode};
use papyrine_ipc::DocId;
use papyrine_writer::{ReplaceOptions, Validation, atomic_replace};

use super::Broker;
use super::engine::base_info;
use crate::api::{DocumentInfo, SaveOptions};
use crate::error::{Code, HostErr, Result};
use crate::session::Session;
use crate::util::{BaseFile, FileStat, file_name};

impl Broker {
    pub fn save(&self, doc_id: &str, opts: &SaveOptions) -> Result<DocumentInfo> {
        let s = self.session(doc_id)?;
        let _g = s.lock_cmd();
        if s.st().closed {
            return Err(HostErr::not_found("document is closed"));
        }
        if s.st().unfinished.is_some() {
            return Err(HostErr::internal(
                "A previous action didn't finish. Choose Redo or Skip before saving.",
            ));
        }
        let (current, password, base, is_ckpt) = {
            let st = s.st();
            (
                st.path.clone(),
                st.password.clone(),
                st.base.clone(),
                st.base_is_checkpoint,
            )
        };
        let target: PathBuf = match (&opts.path, &current) {
            (Some(p), _) => PathBuf::from(p),
            (None, Some(c)) => c.clone(),
            (None, None) => {
                return Err(HostErr::new(
                    Code::Io,
                    "This document has no file yet. Use Save As.",
                ));
            }
        };
        let in_place = current.as_deref().is_some_and(|c| same_file(c, &target));
        let mode = if opts.optimize {
            SaveMode::Optimized {
                break_signatures: opts.break_signatures,
            }
        } else {
            SaveMode::Policy {
                break_signatures: opts.break_signatures,
            }
        };
        let doc = DocId(s.id);
        let payload = match self.with_engine(&s, |c| {
            c.call_blocking(Request::Save {
                doc,
                mode: mode.clone(),
            })
        })? {
            Response::SaveReady(p) => p,
            Response::SaveDecision(d) => return Err(decision_error(d)),
            other => return Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        };
        if payload.unchanged && in_place {
            return Ok(s.info());
        }
        if let Some(why) = &payload.suggest_optimize {
            self.notice("info", "suggest-optimize", why, Some(&s));
        }

        let incremental = payload.kind == SaveKindDto::Incremental;
        let renderer_check = {
            let pw = password.clone();
            move |p: &Path| self.render_check(p, pw.clone())
        };
        let prefix = (in_place && incremental && !is_ckpt)
            .then(|| current.as_deref().map(|c| (c, payload.base_len)))
            .flatten();
        let secret = password.as_deref().map(Secret::from);
        let outcome = atomic_replace(
            &target,
            |tmp| {
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(tmp)?;
                if incremental {
                    // The engine's section extends the file it opened (the on-disk file as
                    // of the last save): copy that prefix, then append.
                    base.copy_prefix(payload.base_len, &mut f)?;
                }
                self.write_delivery(&payload.bytes, &mut f)?;
                Ok(())
            },
            ReplaceOptions {
                validation: Validation {
                    password: secret,
                    expect_pages: Some(payload.page_count as usize),
                    prefix_of: prefix,
                    allow_repairs: false,
                    renderer: Some(&renderer_check),
                },
                faults: None,
            },
        )
        .map_err(|e| {
            HostErr::new(
                Code::Io,
                format!("Save failed: {e}. Your document on disk was not changed."),
            )
        })?;
        if !outcome.metadata_preserved {
            self.notice(
                "warn",
                "metadata-not-preserved",
                "Saved, but some file attributes (such as extended attributes) could not be carried over.",
                Some(&s),
            );
        }
        self.after_save(&s, &target, in_place, payload.token, !incremental)?;
        Ok(s.info())
    }

    fn write_delivery(&self, d: &Delivery, out: &mut impl Write) -> papyrine_writer::Result<()> {
        match d {
            Delivery::Inline(v) => out.write_all(v)?,
            Delivery::File(f) => {
                let dir = self
                    .inner
                    .engine
                    .temp_dir()
                    .ok_or_else(|| std::io::Error::other("the engine is not running"))?;
                let p = crate::util::within(&dir, Path::new(&f.path)).ok_or_else(|| {
                    std::io::Error::other("the engine returned a file outside its temp directory")
                })?;
                let mut src = std::fs::File::open(&p)?;
                let n = std::io::copy(&mut src, out)?;
                let _ = std::fs::remove_file(&p);
                if n != f.len {
                    return Err(std::io::Error::other("engine output has the wrong length").into());
                }
            }
            Delivery::Shared { .. } => {
                return Err(std::io::Error::other("engine used shared memory for a save").into());
            }
        }
        Ok(())
    }

    /// The saved file is the new base for everything: journal, engine, watcher.
    fn after_save(
        &self,
        s: &Arc<Session>,
        target: &Path,
        in_place: bool,
        token: u64,
        optimized: bool,
    ) -> Result<()> {
        let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
        let new_base = Arc::new(BaseFile::open(&target)?);
        let old_path = s.st().path.clone();
        let saved = if optimized {
            Some(new_base.source()?)
        } else {
            None
        };
        let doc = DocId(s.id);
        let fin = match self.with_engine(s, |c| {
            c.call_blocking(Request::SaveCommitted {
                doc,
                token,
                saved: match &saved {
                    // Each attempt needs its own handle.
                    Some(_) => new_base.source().ok(),
                    None => None,
                },
            })
        })? {
            Response::SaveFinished(f) => f,
            other => return Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        };
        if !in_place && let Some(o) = &old_path {
            self.unwatch_path(o);
        }
        // Journal: the saved file is the new base; the log restarts empty.
        let journal = s.st().journal.as_ref().map(|j| j.journal.clone());
        if let Some(j) = journal {
            let r = base_info(&target)
                .map_err(|e| e.to_string())
                .and_then(|b| j.rebase(&b).map_err(|e| e.to_string()));
            if let Err(e) = r {
                self.notice(
                    "warn",
                    "journal-rebase-failed",
                    &format!("Saved, but the recovery journal could not be reset ({e})."),
                    Some(s),
                );
            }
        }
        let owned = {
            let mut st = s.st();
            st.path = Some(target.clone());
            st.name = file_name(&target);
            st.base_path = target.clone();
            st.base = new_base;
            st.dirty = false;
            st.base_is_checkpoint = false;
            if fin.history_reset {
                st.can_undo = false;
                st.can_redo = false;
            }
            if optimized {
                st.repaired = false;
            }
            st.stat = FileStat::of(&target);
            st.external_flagged = false;
            std::mem::take(&mut st.owned_files)
        };
        // A dropped-in temp copy is no longer needed once the user has a real file.
        for f in owned {
            let _ = std::fs::remove_file(f);
        }
        let (action, advice) = self.apply_snapshot(s, fin.snapshot, None)?;
        self.render_sync(s, action)?;
        if let Some(level) = advice {
            self.spawn_compaction(s, level);
        }
        if let Some(n) = fin.notice {
            self.notice("info", "save", &n, Some(s));
        }
        if !in_place || old_path.is_none() {
            self.watch_session(s);
            self.inner.recent.add(&target);
        }
        Ok(())
    }
}

fn decision_error(d: SaveDecision) -> HostErr {
    let (detail, msg) = match d {
        SaveDecision::SignedAndDamaged { reason } => (
            "signed-and-damaged",
            format!(
                "This signed file is damaged ({reason}). Saving it in place would invalidate its signatures. \
                 Save a copy, or save anyway and lose the signatures."
            ),
        ),
        SaveDecision::SignedRewrite => (
            "signed-rewrite",
            "Optimizing a signed file invalidates its signatures. Continue only if that is acceptable.".to_string(),
        ),
    };
    let mut e = HostErr::new(Code::Internal, msg);
    e.detail = Some(detail.into());
    e
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}
