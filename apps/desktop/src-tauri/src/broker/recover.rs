//! Launch-time recovery, the external-change watcher hooks and action prompts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Broker;
use super::edit::label_for;
use super::open::OpenSpec;
use crate::api::{DocumentInfo, HostEvent};
use crate::error::{HostErr, Result};
use crate::recovery::{MODE_COPY, MODE_REFUSED, MODE_RESTORE, Offer, UnfinishedInfo};
use crate::session::{JournalHandle, Session, Unfinished};
use crate::util::{FileStat, file_name, now_ms};
use papyrine_journal::{Journal, Recovery, SystemClock, recover};

/// Result of restoring: the document plus a pending "last action didn't finish" prompt.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub info: DocumentInfo,
    pub unfinished: Option<UnfinishedInfo>,
}

impl Broker {
    // -------------------------------------------------------------- launch scan

    /// Look through `recovery/`, drop leftovers with nothing to restore, prune old
    /// directories and announce what is left. Runs after first paint.
    pub fn scan_recovery(&self) -> Vec<Offer> {
        let root = &self.inner.recovery;
        let pruned = root.prune(now_ms(), papyrine_journal::PRUNE_AFTER_DAYS);
        if !pruned.is_empty() {
            let names: Vec<String> = pruned
                .iter()
                .map(|p| p.original_path.clone().unwrap_or_else(|| p.doc_id.clone()))
                .collect();
            self.notice(
                "info",
                "recovery-pruned",
                &format!(
                    "Removed unsaved-changes backups older than {} days: {}",
                    papyrine_journal::PRUNE_AFTER_DAYS,
                    names.join(", ")
                ),
                None,
            );
        }
        let open_keys: Vec<String> = self.sessions().iter().map(|s| s.doc_key.clone()).collect();
        let mut offers = Vec::new();
        for f in root.scan() {
            if open_keys.contains(&f.doc_id) {
                continue;
            }
            let rec = match recover(self.inner.fs.as_ref(), &f.dir) {
                Ok(r) => r,
                Err(_) => continue, // unreadable: left for the 30-day prune
            };
            if rec.clean || rec.is_empty() {
                let _ = self.inner.fs.remove_dir_all(&f.dir);
                continue;
            }
            offers.push(self.make_offer(&f.doc_id, &f.dir, &rec));
        }
        *self.inner.offers.lock().unwrap_or_else(|p| p.into_inner()) = offers.clone();
        if !offers.is_empty() {
            self.emit(HostEvent::RecoveryAvailable {
                entries: self.recovery_entries(),
            });
        }
        offers
    }

    pub fn recovery_offers(&self) -> Vec<Offer> {
        self.inner
            .offers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn make_offer(&self, id: &str, dir: &Path, rec: &Recovery) -> Offer {
        let original = PathBuf::from(&rec.base.original_path);
        let matches = rec.base.original_still_matches();
        let ckpt_ok = rec.checkpoint.as_ref().is_some_and(|c| c.blob_ok);
        let (mode, explanation) = if rec.checkpoint.as_ref().is_some_and(|c| !c.blob_ok) {
            (
                MODE_REFUSED,
                Some("The saved recovery data is damaged, so your unsaved changes cannot be restored.".to_string()),
            )
        } else if matches {
            (MODE_RESTORE, None)
        } else if ckpt_ok {
            (
                MODE_COPY,
                Some("The original file changed after your edits, so they cannot be applied to it. A recovered copy of your work can be opened instead.".to_string()),
            )
        } else {
            (
                MODE_REFUSED,
                Some("The original file changed (or is gone) since your unsaved edits, so they cannot be applied safely.".to_string()),
            )
        };
        let saved_at_ms = self
            .inner
            .fs
            .stat(&dir.join("journal.log"))
            .map(|s| s.mtime_ms)
            .unwrap_or(rec.base.mtime_ms);
        let unfinished = rec.unfinished.first().map(|u| UnfinishedInfo {
            command: u.command.clone(),
            label: label_for(&u.command, &u.params),
            prior_crashes: u.prior_crashes,
        });
        Offer {
            id: id.to_string(),
            name: display_name(&original),
            original_path: rec.base.original_path.clone(),
            saved_at_ms,
            edits: rec.commits.len() + usize::from(rec.checkpoint.is_some()),
            mode: mode.into(),
            explanation,
            unfinished,
        }
    }

    // ------------------------------------------------------------------ restore

    /// Restore (`mode` "restore") or open the recovered copy (`mode` "recovered-copy").
    pub fn restore_recovery(&self, id: &str, mode: &str) -> Result<Restored> {
        let dir = self
            .inner
            .recovery
            .doc_dir(id)
            .map_err(|e| HostErr::internal(e.to_string()))?;
        if !dir.exists() {
            return Err(HostErr::not_found("That recovery data no longer exists."));
        }
        let rec = recover(self.inner.fs.as_ref(), &dir)?;
        let offer = self.make_offer(id, &dir, &rec);
        let want_copy = match mode {
            MODE_RESTORE => false,
            MODE_COPY => true,
            other => {
                return Err(HostErr::internal(format!(
                    "unknown recovery mode {other:?}"
                )));
            }
        };
        if offer.mode == MODE_REFUSED {
            return Err(HostErr::internal(
                offer
                    .explanation
                    .unwrap_or_else(|| "Recovery was refused.".into()),
            ));
        }
        if want_copy != (offer.mode == MODE_COPY) {
            return Err(HostErr::internal(format!(
                "This recovery can only be opened as: {}",
                offer.mode
            )));
        }

        let (journal, rec) = Journal::open(
            &self.inner.recovery,
            self.inner.fs.clone(),
            id,
            self.inner.cfg.journal.clone(),
            Arc::new(SystemClock),
        )?;
        let flusher = journal.spawn_flusher();
        let original = PathBuf::from(&rec.base.original_path);
        let untitled = self.inner.cfg.data_dir.join("untitled");
        let is_untitled = original.starts_with(&untitled);
        // The file the engine opens: a checkpoint (full file) if there is one.
        let (base_path, owned, base_is_checkpoint) = match &rec.checkpoint {
            Some(c) => {
                let bytes = journal.read_blob(&c.blob)?;
                let dir = self.inner.cfg.data_dir.join("untitled");
                std::fs::create_dir_all(&dir)?;
                let f = dir.join(format!("recovered-{id}.pdf"));
                std::fs::write(&f, bytes)?;
                (f.clone(), vec![f], true)
            }
            None => (original.clone(), vec![], false),
        };
        let mut owned = owned;
        let (path, name) = if want_copy || (is_untitled && !base_is_checkpoint) {
            if is_untitled && !want_copy {
                owned.push(original.clone());
            }
            (
                None,
                format!(
                    "{} (recovered).pdf",
                    display_name(&original).trim_end_matches(".pdf")
                ),
            )
        } else {
            (Some(original.clone()), file_name(&original))
        };
        let name = if is_untitled && !want_copy {
            display_name(&original)
        } else {
            name
        };
        let s = self.open_session(OpenSpec {
            path,
            base_path,
            name,
            owned_files: owned,
            password: None,
            journal: Some(JournalHandle {
                journal: journal.clone(),
                _flusher: flusher,
            }),
            doc_key: Some(id.to_string()),
            base_is_checkpoint,
        })?;
        if let Some(u) = rec.unfinished.first() {
            let mut st = s.st();
            st.unfinished = Some(Unfinished {
                seq: u.seq,
                command: u.command.clone(),
                label: label_for(&u.command, &u.params),
                prior_crashes: u.prior_crashes,
                blobs: u.blobs.clone(),
            });
            st.unfinished_params = Some(u.params.clone());
        }
        self.inner
            .offers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|o| o.id != id);
        if let Some(p) = s.st().path.clone() {
            self.inner.recent.add(&p);
        }
        let unfinished = rec.unfinished.first().map(|u| UnfinishedInfo {
            command: u.command.clone(),
            label: label_for(&u.command, &u.params),
            prior_crashes: u.prior_crashes,
        });
        Ok(Restored {
            info: s.info(),
            unfinished,
        })
    }

    /// Throw away a recovery directory ("Discard").
    pub fn discard_recovery(&self, id: &str) -> Result<()> {
        let dir = self
            .inner
            .recovery
            .doc_dir(id)
            .map_err(|e| HostErr::internal(e.to_string()))?;
        if let Ok(rec) = recover(self.inner.fs.as_ref(), &dir) {
            let original = PathBuf::from(&rec.base.original_path);
            if original.starts_with(self.inner.cfg.data_dir.join("untitled")) {
                let _ = std::fs::remove_file(original);
            }
        }
        match self.inner.fs.remove_dir_all(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        self.inner
            .offers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|o| o.id != id);
        Ok(())
    }

    // ------------------------------------------------------------ external changes

    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    pub(crate) fn watch_session(&self, s: &Session) {
        let Some(Some(w)) = self.inner.watch.get() else {
            return;
        };
        if let Some(p) = s.st().path.clone() {
            w.watch(&Self::canon(&p));
        }
    }

    pub(crate) fn unwatch_session(&self, s: &Session) {
        if let Some(p) = s.st().path.clone() {
            self.unwatch_path(&p);
        }
    }

    pub(crate) fn unwatch_path(&self, p: &Path) {
        if let Some(Some(w)) = self.inner.watch.get() {
            w.unwatch(&Self::canon(p));
        }
    }

    /// A file-system event for `path` (debounced): compare mtime/size per document.
    pub(crate) fn on_fs_event(&self, path: &Path) {
        let ev = Self::canon(path);
        for s in self.sessions() {
            let Some(p) = s.st().path.clone() else {
                continue;
            };
            // The file may be gone (editor replaced it), so compare file names under the
            // canonical directory.
            let sp = Self::canon(&p);
            if sp == ev || (sp.file_name() == ev.file_name() && sp.parent() == ev.parent()) {
                self.check_external(&s);
            }
        }
    }

    /// Has the file changed under us? Emits `file-changed-on-disk` once per change.
    pub fn check_external(&self, s: &Session) -> bool {
        let (path, known, flagged) = {
            let st = s.st();
            (st.path.clone(), st.stat, st.external_flagged)
        };
        let Some(path) = path else { return false };
        if flagged {
            return true;
        }
        let now = FileStat::of(&path);
        if now != known {
            s.st().external_flagged = true;
            self.emit(HostEvent::FileChangedOnDisk { doc_id: s.doc_id() });
            return true;
        }
        false
    }

    /// Window focus: re-check every open file (cheap stat calls).
    pub fn check_external_all(&self) {
        for s in self.sessions() {
            self.check_external(&s);
        }
    }

    /// The user chose "Keep mine" or dismissed the banner: stop reporting this change.
    pub fn acknowledge_external(&self, doc_id: &str) -> Result<()> {
        let s = self.session(doc_id)?;
        let mut st = s.st();
        st.stat = st.path.as_deref().and_then(FileStat::of);
        st.external_flagged = false;
        Ok(())
    }
}

/// File name without the random prefix `open_bytes` adds (`<8 hex>-name.pdf`).
fn display_name(p: &Path) -> String {
    let n = file_name(p);
    match n.split_once('-') {
        Some((pre, rest)) if pre.len() == 8 && pre.chars().all(|c| c.is_ascii_hexdigit()) => {
            rest.to_string()
        }
        _ => n,
    }
}
