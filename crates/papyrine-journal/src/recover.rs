//! Reading and replaying a journal.

use crate::dir::{BASE_FILE, BaseInfo, LOG_FILE, Quarantine, Quarantined, blob_path, command_key};
use crate::error::{JournalError, Result};
use crate::fs::Fs;
use crate::record::{AfterImage, BlobRef, ObjId, Record, Stop, scan};
use std::collections::BTreeMap;
use std::path::Path;

/// An Intent with its Commit: apply the after-images; never re-run the command.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedCommand {
    pub seq: u64,
    pub ts_ms: u64,
    pub command: String,
    pub params: serde_json::Value,
    pub blobs: Vec<BlobRef>,
    pub after_images: Vec<AfterImage>,
    pub created: Vec<ObjId>,
}

/// An Intent with no Commit: the process died during apply. Ask the user: Redo / Skip.
#[derive(Debug, Clone, PartialEq)]
pub struct UnfinishedCommand {
    pub seq: u64,
    pub ts_ms: u64,
    pub command: String,
    pub params: serde_json::Value,
    pub blobs: Vec<BlobRef>,
    /// Crashes already counted against this exact command (name + params + inputs).
    pub prior_crashes: u32,
}

impl UnfinishedCommand {
    /// "Your last action, X, didn't finish."
    pub fn message(&self) -> String {
        format!("Your last action, {}, didn't finish.", self.command)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub upto_seq: u64,
    pub blob: BlobRef,
    /// Whether the blob file exists and matches its hash.
    pub blob_ok: bool,
}

#[derive(Debug, Clone)]
pub struct Recovery {
    pub base: BaseInfo,
    pub checkpoint: Option<CheckpointInfo>,
    /// Commands to apply, in order, on top of the checkpoint (or the original).
    pub commits: Vec<CommittedCommand>,
    /// Intents that never committed and were never resolved.
    pub unfinished: Vec<UnfinishedCommand>,
    /// Intents resolved without Commit (clean failure, skipped, redone).
    pub abandoned: Vec<u64>,
    /// Commit records without a matching Intent (should never happen; reported).
    pub orphan_commits: u32,
    /// The log ended in a Clean marker: nothing to restore.
    pub clean: bool,
    /// Bytes cut from a torn or corrupt tail.
    pub discarded_bytes: u64,
    pub tail: Stop,
    pub quarantined: Vec<Quarantined>,
    /// Highest seq seen; the next intent gets `last_seq + 1`.
    pub last_seq: u64,
    /// Records in the (valid) log, for checkpoint accounting.
    pub record_count: u64,
    pub valid_len: u64,
}

impl Recovery {
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty() && self.unfinished.is_empty() && self.checkpoint.is_none()
    }
}

/// Read-only recovery of the directory `dir`. Does not touch any file.
pub fn recover(fs: &dyn Fs, dir: &Path) -> Result<Recovery> {
    let base: BaseInfo = serde_json::from_slice(
        &fs.read(&dir.join(BASE_FILE))
            .map_err(|e| JournalError::Corrupt(format!("base.json: {e}")))?,
    )?;
    let log = match fs.read(&dir.join(LOG_FILE)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let sc = if log.is_empty() {
        crate::record::Scan {
            records: vec![],
            valid_len: 0,
            stop: Stop::End,
        }
    } else {
        scan(&log)?
    };
    let quarantine = Quarantine::load(fs, dir);

    let mut checkpoint: Option<CheckpointInfo> = None;
    let mut open: BTreeMap<u64, (u64, String, serde_json::Value, Vec<BlobRef>)> = BTreeMap::new();
    let mut commits: Vec<CommittedCommand> = Vec::new();
    let mut abandoned = Vec::new();
    let mut orphan_commits = 0;
    let mut clean = false;
    let mut last_seq = 0;
    for rec in &sc.records {
        match rec {
            Record::Intent {
                seq,
                ts_ms,
                command,
                params,
                blobs,
            } => {
                clean = false;
                last_seq = last_seq.max(*seq);
                open.insert(
                    *seq,
                    (*ts_ms, command.clone(), params.clone(), blobs.clone()),
                );
            }
            Record::Commit {
                seq,
                after_images,
                created,
                ..
            } => match open.remove(seq) {
                Some((ts_ms, command, params, blobs)) => commits.push(CommittedCommand {
                    seq: *seq,
                    ts_ms,
                    command,
                    params,
                    blobs,
                    after_images: after_images.clone(),
                    created: created.clone(),
                }),
                None => orphan_commits += 1,
            },
            Record::Abandon { seq, .. } => {
                open.remove(seq);
                abandoned.push(*seq);
            }
            Record::Checkpoint { upto_seq, blob, .. } => {
                commits.clear();
                open.clear();
                abandoned.clear();
                last_seq = last_seq.max(*upto_seq);
                let ok = fs
                    .read(&blob_path(dir, &blob.hash))
                    .map(|b| {
                        b.len() as u64 == blob.len && blake3::hash(&b).as_bytes() == &blob.hash
                    })
                    .unwrap_or(false);
                checkpoint = Some(CheckpointInfo {
                    upto_seq: *upto_seq,
                    blob: *blob,
                    blob_ok: ok,
                });
            }
            Record::Clean { .. } => {
                clean = true;
                checkpoint = None;
                open.clear();
                commits.clear();
                abandoned.clear();
            }
        }
    }
    let unfinished = open
        .into_iter()
        .map(|(seq, (ts_ms, command, params, blobs))| {
            let prior_crashes = quarantine.crashes(&command_key(&command, &params, &blobs));
            UnfinishedCommand {
                seq,
                ts_ms,
                command,
                params,
                blobs,
                prior_crashes,
            }
        })
        .collect();
    Ok(Recovery {
        base,
        checkpoint,
        commits,
        unfinished,
        abandoned,
        orphan_commits,
        clean,
        discarded_bytes: (log.len() as u64).saturating_sub(sc.valid_len),
        tail: sc.stop,
        quarantined: quarantine.list(),
        last_seq,
        record_count: sc.records.len() as u64,
        valid_len: sc.valid_len,
    })
}
