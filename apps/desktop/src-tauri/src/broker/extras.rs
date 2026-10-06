//! The rest of the UI contract that is host business rather than engine business:
//! recovery entries, reload from disk, action prompts, pickers, recent files, preferences.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use papyrine_engine::proto::{OpenParams, Request, Response};
use papyrine_ipc::DocId;

use super::Broker;
use super::recover::Restored;
use crate::api::{
    DocumentInfo, HostEvent, OpenSourceJson, PickedFile, RecoveryEntry, UnfinishedTitle,
};
use crate::error::{Code, HostErr, Result};
use crate::recovery::{MODE_COPY, MODE_RESTORE};
use crate::util::{BaseFile, FileStat, file_name, iso_from_ms};

/// A URI or Launch action waiting for the user's answer.
#[derive(Clone, Debug)]
pub(crate) struct Prompt {
    pub doc_id: String,
    pub kind: String,
    pub target: String,
}

impl Broker {
    // ------------------------------------------------------------------- recovery

    pub fn recovery_entries(&self) -> Vec<RecoveryEntry> {
        self.recovery_offers()
            .into_iter()
            .map(|o| {
                let untitled = Path::new(&o.original_path)
                    .starts_with(self.inner.cfg.data_dir.join("untitled"));
                RecoveryEntry {
                    id: o.id,
                    name: o.name,
                    path: (!untitled).then_some(o.original_path),
                    last_activity: iso_from_ms(o.saved_at_ms),
                    state: if o.mode == MODE_RESTORE {
                        "restorable".into()
                    } else {
                        "original-changed".into()
                    },
                    unfinished: o.unfinished.map(|u| UnfinishedTitle { title: u.label }),
                }
            })
            .collect()
    }

    /// `restore`, `open-copy` or `discard` for a recovery entry.
    pub fn recover(&self, id: &str, action: &str) -> Result<Option<DocumentInfo>> {
        match action {
            "restore" => Ok(Some(self.restore_recovery(id, MODE_RESTORE)?.info)),
            "open-copy" => Ok(Some(self.restore_recovery(id, MODE_COPY)?.info)),
            "discard" => {
                self.discard_recovery(id)?;
                Ok(None)
            }
            other => Err(HostErr::internal(format!(
                "unknown recovery action {other:?}"
            ))),
        }
    }

    /// Restore and also report the pending "last action didn't finish" prompt.
    pub fn restore_with_prompt(&self, id: &str, mode: &str) -> Result<Restored> {
        self.restore_recovery(id, mode)
    }

    /// Redo or skip the unfinished command of the document restored from recovery `id`.
    pub fn resolve_unfinished_for(&self, id: &str, redo: bool) -> Result<()> {
        let s = self
            .sessions()
            .into_iter()
            .find(|s| s.doc_key == id)
            .ok_or_else(|| HostErr::not_found("That recovered document is not open."))?;
        self.resolve_unfinished(&s.doc_id(), redo)?;
        Ok(())
    }

    // ------------------------------------------------------------ reload from disk

    /// Banner "Reload": throw the in-memory state away and read the file again.
    pub fn reload_from_disk(&self, doc_id: &str) -> Result<DocumentInfo> {
        let s = self.session(doc_id)?;
        let _g = s.lock_cmd();
        let path = s
            .st()
            .path
            .clone()
            .ok_or_else(|| HostErr::new(Code::Io, "This document has no file to reload."))?;
        let new_base = std::sync::Arc::new(BaseFile::open(&path)?);
        if let Some(j) = s.st().journal.take() {
            let _ = j.journal.discard();
        }
        // Close the stale copies first, then reopen from the file.
        let doc = DocId(s.id);
        if self.inner.engine.is_running()
            && let Ok(c) = self.inner.engine.conn()
        {
            let _ = c.client.call_blocking(Request::Close { doc });
        }
        {
            let mut st = s.st();
            st.base = new_base;
            st.base_path = path.clone();
            st.render_base = None;
            st.sections.clear();
            st.snapshot_epoch = 0;
            st.engine_gen = 0;
            st.render_gen = 0;
            st.dirty = false;
            st.can_undo = false;
            st.can_redo = false;
            st.unfinished = None;
            st.unfinished_params = None;
            st.base_is_checkpoint = false;
            st.journal_failed = false;
            st.checkpoint_disabled = false;
            st.stat = FileStat::of(&path);
            st.external_flagged = false;
            st.revision += 1;
            st.epoch += 1;
        }
        let summary = self.engine_first_open(&s)?;
        if summary.repaired {
            self.notice(
                "info",
                "repaired",
                "This file was damaged and has been repaired in memory.",
                Some(&s),
            );
        }
        self.render_sync(&s, super::engine::RenderAction::Rebase)?;
        self.invalidate_doc(s.id);
        let info = s.info();
        let n = info.page_count;
        self.emit(HostEvent::DocumentChanged {
            info: info.clone(),
            invalidated_pages: (0..n).collect(),
        });
        Ok(info)
    }

    // ------------------------------------------------------------ action prompts

    /// A document asked for a URI or Launch action. The host never performs it silently:
    /// it tells the UI to show the target and ask. Returns the prompt id.
    pub fn request_action(&self, doc_id: &str, kind: &str, target: &str) -> Result<String> {
        self.session(doc_id)?;
        let kind = match kind {
            "uri" | "launch" => kind,
            other => {
                return Err(HostErr::internal(format!(
                    "unsupported action kind {other:?}"
                )));
            }
        };
        let target: String = target
            .chars()
            .filter(|c| !c.is_control())
            .take(2048)
            .collect();
        let prompt_id = format!(
            "prompt-{}",
            self.inner.temp_ids.fetch_add(1, Ordering::Relaxed)
        );
        self.inner
            .prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                prompt_id.clone(),
                Prompt {
                    doc_id: doc_id.to_string(),
                    kind: kind.to_string(),
                    target: target.clone(),
                },
            );
        self.emit(HostEvent::ActionPrompt {
            doc_id: doc_id.to_string(),
            prompt_id: prompt_id.clone(),
            kind: kind.to_string(),
            target,
        });
        Ok(prompt_id)
    }

    /// The user's answer. Only `allow` on a web or mail link does anything; a Launch action
    /// is shown but never run.
    pub fn resolve_action_prompt(&self, doc_id: &str, prompt_id: &str, allow: bool) -> Result<()> {
        let p = self
            .inner
            .prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(prompt_id)
            .ok_or_else(|| HostErr::not_found("That prompt has already been answered."))?;
        if p.doc_id != doc_id || !allow {
            return Ok(());
        }
        if p.kind == "launch" {
            return Err(HostErr::new(
                Code::Internal,
                "Papyrine never runs programs named by a document.",
            ));
        }
        self.open_uri(&p.target)
    }

    /// Open a URI after the user confirmed. Only http(s) and mailto; never a file or program.
    pub(crate) fn open_uri(&self, target: &str) -> Result<()> {
        let lower = target.to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:"))
        {
            return Err(HostErr::new(
                Code::Io,
                "Only web and mail links can be opened.",
            ));
        }
        if target.chars().any(|c| c.is_control() || c == ' ') {
            return Err(HostErr::new(Code::Io, "That link is not valid."));
        }
        #[cfg(target_os = "macos")]
        let mut cmd = std::process::Command::new("open");
        #[cfg(target_os = "windows")]
        let mut cmd = {
            // `rundll32 url.dll,FileProtocolHandler` takes the URL as one argument, no shell.
            let mut c = std::process::Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(target);
        cmd.spawn()?;
        Ok(())
    }

    // ------------------------------------------------------------- open requests

    /// OS file association, second launch or drag and drop asked for these files. Until the
    /// UI has attached (`take_pending_opens`) they are queued so none is lost.
    pub fn request_open(&self, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
        if paths.is_empty() {
            return;
        }
        {
            let mut q = self
                .inner
                .pending_opens
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if let Some(v) = q.as_mut() {
                v.extend(paths);
                return;
            }
        }
        self.emit(HostEvent::OpenRequested {
            sources: paths
                .iter()
                .map(|p| OpenSourceJson::Path {
                    path: p.to_string_lossy().into_owned(),
                })
                .collect(),
        });
    }

    /// The UI is listening: hand over what arrived earlier and switch to live events.
    pub fn take_pending_opens(&self) -> Vec<PathBuf> {
        self.inner
            .pending_opens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or_default()
    }

    // -------------------------------------------------------------------- pickers

    /// Native picker for PDFs the UI needs paths of (insert, merge).
    pub fn pick_pdfs(&self, multiple: bool) -> Result<Vec<PickedFile>> {
        let mut paths = self.inner.cfg.dialogs.open();
        if !multiple {
            paths.truncate(1);
        }
        paths.iter().map(|p| self.describe_pdf(p)).collect()
    }

    fn describe_pdf(&self, p: &Path) -> Result<PickedFile> {
        let meta = std::fs::metadata(p)?;
        // Ask the sandboxed engine for the page count; the host never parses PDFs.
        let page_count = match self.probe_page_count(p) {
            Ok(n) => n,
            Err(e) if e.code == Code::Encrypted => 0,
            Err(e) => return Err(e),
        };
        Ok(PickedFile {
            path: p.to_string_lossy().into_owned(),
            name: file_name(p),
            page_count,
            size: meta.len(),
        })
    }

    fn probe_page_count(&self, path: &Path) -> Result<usize> {
        let c = self.inner.engine.conn()?;
        let f = BaseFile::open(path)?;
        let doc = DocId(u64::MAX - self.inner.temp_ids.fetch_add(1, Ordering::Relaxed));
        let r = c.client.call_blocking(Request::Open {
            doc,
            source: f.source()?,
            password: None,
            params: OpenParams {
                skip_pages: true,
                spill_budget_bytes: None,
            },
        });
        let n = match r? {
            Response::Opened(s) => s.page_count as usize,
            other => return Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
        };
        let _ = c.client.call_blocking(Request::Close { doc });
        Ok(n)
    }

    /// Native save dialog (or folder picker).
    pub fn pick_save_path(&self, suggested: &str, directory: bool) -> Option<PathBuf> {
        if directory {
            self.inner.cfg.dialogs.folder()
        } else {
            self.inner.cfg.dialogs.save(suggested)
        }
    }

    pub fn clear_recent_files(&self) {
        self.inner.recent.clear();
    }

    pub fn disable_optimize_suggestion(&self) {
        self.inner.prefs.update(|p| p.optimize_suggestion = false);
    }

    pub fn optimize_suggestion_enabled(&self) -> bool {
        self.inner.prefs.get().optimize_suggestion
    }

    pub fn privacy(&self) -> serde_json::Value {
        serde_json::json!({
            "updateCheck": self.inner.prefs.get().update_check,
            "lockedByPolicy": false,
        })
    }

    pub fn set_update_check(&self, choice: &str) -> Result<serde_json::Value> {
        if choice != "on" && choice != "off" {
            return Err(HostErr::internal("the update check is either on or off"));
        }
        self.inner
            .prefs
            .update(|p| p.update_check = choice.to_string());
        Ok(self.privacy())
    }
}
