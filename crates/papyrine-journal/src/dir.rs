//! Recovery directory layout: `<root>/<doc-id>/{base.json, journal.log, blobs/, quarantine.json}`.

use crate::error::{JournalError, Result};
use crate::fs::Fs;
use crate::record::{Hash, hash_hex};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const BASE_FILE: &str = "base.json";
pub const LOG_FILE: &str = "journal.log";
pub const BLOB_DIR: &str = "blobs";
pub const QUARANTINE_FILE: &str = "quarantine.json";
pub const PRUNE_AFTER_DAYS: u64 = 30;

/// What the journal was started against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseInfo {
    pub original_path: String,
    pub size: u64,
    /// Unix milliseconds.
    pub mtime_ms: u64,
    /// BLAKE3 of the original file, lowercase hex.
    pub blake3: String,
}

impl BaseInfo {
    /// Hash and stat the file on disk.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let mut f = std::fs::File::open(path)?;
        let meta = f.metadata()?;
        let mut h = blake3::Hasher::new();
        h.update_reader(&mut f)?;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Ok(Self {
            original_path: path.to_string_lossy().into_owned(),
            size: meta.len(),
            mtime_ms,
            blake3: h.finalize().to_hex().to_string(),
        })
    }

    /// Does the file currently on disk still match (size, then hash)? Used to choose between
    /// "Restore unsaved changes" and "Open recovered copy".
    pub fn original_still_matches(&self) -> bool {
        match Self::from_file(Path::new(&self.original_path)) {
            Ok(now) => now.size == self.size && now.blake3 == self.blake3,
            Err(_) => false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct QEntry {
    pub key: String,
    pub command: String,
    pub crashes: u32,
    /// Intent seqs already counted (makes counting idempotent across a crash).
    pub seqs: Vec<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Quarantine {
    pub entries: Vec<QEntry>,
}

/// Public view of a quarantine entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantined {
    pub command: String,
    pub crashes: u32,
}

pub const CRASH_LIMIT: u32 = 2;

impl Quarantine {
    pub fn load(fs: &dyn Fs, dir: &Path) -> Quarantine {
        fs.read(&dir.join(QUARANTINE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, fs: &dyn Fs, dir: &Path) -> Result<()> {
        fs.write_atomic(&dir.join(QUARANTINE_FILE), &serde_json::to_vec(self)?)?;
        Ok(())
    }
    pub fn crashes(&self, key: &str) -> u32 {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map_or(0, |e| e.crashes)
    }
    /// Count a crash for `seq`; returns true if the counter changed.
    pub fn bump(&mut self, key: &str, command: &str, seq: u64) -> bool {
        let i = match self.entries.iter().position(|e| e.key == key) {
            Some(i) => i,
            None => {
                self.entries.push(QEntry {
                    key: key.into(),
                    command: command.into(),
                    ..Default::default()
                });
                self.entries.len() - 1
            }
        };
        let e = &mut self.entries[i];
        if e.seqs.contains(&seq) {
            return false;
        }
        e.seqs.push(seq);
        e.crashes += 1;
        true
    }
    pub fn list(&self) -> Vec<Quarantined> {
        self.entries
            .iter()
            .filter(|e| e.crashes >= CRASH_LIMIT)
            .map(|e| Quarantined {
                command: e.command.clone(),
                crashes: e.crashes,
            })
            .collect()
    }
}

/// Identity of "the same command" for quarantine: name + params + blob hashes.
pub fn command_key(
    command: &str,
    params: &serde_json::Value,
    blobs: &[crate::record::BlobRef],
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(command.as_bytes());
    h.update(&[0]);
    h.update(serde_json::to_string(params).unwrap_or_default().as_bytes());
    for b in blobs {
        h.update(&b.hash);
    }
    h.finalize().to_hex().to_string()
}

pub(crate) fn blob_path(dir: &Path, h: &Hash) -> PathBuf {
    dir.join(BLOB_DIR).join(hash_hex(h))
}

/// The per-user `recovery/` directory.
#[derive(Clone)]
pub struct RecoveryRoot {
    fs: Arc<dyn Fs>,
    root: PathBuf,
}

/// A recovery dir found by `RecoveryRoot::scan`.
#[derive(Debug, Clone)]
pub struct Found {
    pub doc_id: String,
    pub dir: PathBuf,
    pub base: Option<BaseInfo>,
}

/// A directory removed by `prune`.
#[derive(Debug, Clone)]
pub struct Pruned {
    pub doc_id: String,
    pub age_days: u64,
    pub original_path: Option<String>,
}

impl RecoveryRoot {
    pub fn new(fs: Arc<dyn Fs>, root: impl Into<PathBuf>) -> Self {
        Self {
            fs,
            root: root.into(),
        }
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn doc_dir(&self, doc_id: &str) -> Result<PathBuf> {
        if doc_id.is_empty() || doc_id.contains(['/', '\\']) || doc_id.starts_with('.') {
            return Err(JournalError::Corrupt(format!("invalid doc id {doc_id:?}")));
        }
        Ok(self.root.join(doc_id))
    }

    /// Recovery dirs present at launch (those with a readable base.json or a log).
    pub fn scan(&self) -> Vec<Found> {
        let mut out = Vec::new();
        for e in self.fs.list_dir(&self.root).unwrap_or_default() {
            if !e.is_dir {
                continue;
            }
            let base = self
                .fs
                .read(&e.path.join(BASE_FILE))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            if base.is_some() || self.fs.stat(&e.path.join(LOG_FILE)).is_ok() {
                out.push(Found {
                    doc_id: e.name,
                    dir: e.path,
                    base,
                });
            }
        }
        out.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
        out
    }

    /// Remove recovery dirs untouched for more than `max_age_days`. The caller shows the
    /// returned list as a notice.
    pub fn prune(&self, now_ms: u64, max_age_days: u64) -> Vec<Pruned> {
        let mut out = Vec::new();
        for e in self.fs.list_dir(&self.root).unwrap_or_default() {
            if !e.is_dir {
                continue;
            }
            let newest = [
                e.path.clone(),
                e.path.join(LOG_FILE),
                e.path.join(BASE_FILE),
            ]
            .iter()
            .filter_map(|p| self.fs.stat(p).ok())
            .map(|s| s.mtime_ms)
            .max()
            .unwrap_or(0);
            let age_days = now_ms.saturating_sub(newest) / 86_400_000;
            if age_days > max_age_days {
                let original_path = self
                    .fs
                    .read(&e.path.join(BASE_FILE))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<BaseInfo>(&b).ok())
                    .map(|b| b.original_path);
                if self.fs.remove_dir_all(&e.path).is_ok() {
                    out.push(Pruned {
                        doc_id: e.name,
                        age_days,
                        original_path,
                    });
                }
            }
        }
        out
    }
}
