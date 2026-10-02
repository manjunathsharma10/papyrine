//! The writer side: write-ahead append, group fsync, checkpoints, resolution of unfinished
//! intents, quarantine.

use crate::dir::{
    BASE_FILE, BLOB_DIR, BaseInfo, CRASH_LIMIT, LOG_FILE, Quarantine, RecoveryRoot, blob_path,
    command_key,
};
use crate::error::{JournalError, Result};
use crate::fs::{Fs, FsFile};
use crate::record::{
    AfterImage, BlobRef, Compression, Hash, MAGIC, ObjId, Record, commit_digest, encode, hash_hex,
};
use crate::recover::{Recovery, recover};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// Source of "now" in unix milliseconds. Injectable for tests.
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Group-commit window; clamped to 0.2 to 5 s.
    pub sync_interval: Duration,
    /// Records or blobs above this wait for their own fsync before the call returns.
    pub large_payload_bytes: usize,
    pub checkpoint_bytes: u64,
    pub checkpoint_records: u64,
    /// After-images above this are zstd-compressed.
    pub zstd_threshold: usize,
    pub zstd_level: i32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sync_interval: Duration::from_secs(1),
            large_payload_bytes: 1 << 20,
            checkpoint_bytes: 32 << 20,
            checkpoint_records: 500,
            zstd_threshold: 4096,
            zstd_level: 3,
        }
    }
}

impl Options {
    fn interval_ms(&self) -> u64 {
        (self.sync_interval.as_millis() as u64).clamp(200, 5000)
    }
    fn comp(&self) -> Compression {
        Compression {
            threshold: self.zstd_threshold,
            level: self.zstd_level,
        }
    }
}

/// Proof that an `Intent` was written. Not `Clone`, only `begin` makes one.
#[derive(Debug)]
pub struct IntentToken {
    seq: u64,
}
impl IntentToken {
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// How the host resolves an unfinished intent after a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Skip,
    /// The caller will re-issue the command with `begin`; recorded the same way.
    Redo,
}

struct State {
    file: Arc<dyn FsFile>,
    good_len: u64,
    next_seq: u64,
    inflight: Option<u64>,
    /// Intents (from a previous process) awaiting Redo/Skip, with their quarantine keys.
    unresolved: HashMap<u64, (String, String)>,
    keys: HashMap<u64, (String, String)>,
    written_gen: u64,
    synced_gen: u64,
    dirty_since: Option<u64>,
    broken: Option<String>,
    recs: u64,
    bytes: u64,
    quarantine: Quarantine,
    closed: bool,
}

struct Inner {
    fs: Arc<dyn Fs>,
    clock: Arc<dyn Clock>,
    opts: Options,
    dir: PathBuf,
    state: Mutex<State>,
}

/// A per-document write-ahead journal. Cheap to clone (shared handle).
#[derive(Clone)]
pub struct Journal {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Journal({})", self.inner.dir.display())
    }
}

fn header_file() -> Vec<u8> {
    MAGIC.to_vec()
}

impl Journal {
    /// Start a fresh journal for a newly opened document.
    pub fn create(
        root: &RecoveryRoot,
        fs: Arc<dyn Fs>,
        doc_id: &str,
        base: &BaseInfo,
        opts: Options,
        clock: Arc<dyn Clock>,
    ) -> Result<Journal> {
        let dir = root.doc_dir(doc_id)?;
        fs.create_dir_all(&dir.join(BLOB_DIR))?;
        let _ = fs.exclude_from_backup(root.path());
        fs.write_atomic(&dir.join(LOG_FILE), &header_file())?;
        fs.write_atomic(&dir.join(BASE_FILE), &serde_json::to_vec_pretty(base)?)?;
        let file: Arc<dyn FsFile> = fs.open_append(&dir.join(LOG_FILE))?.into();
        let state = State {
            file,
            good_len: MAGIC.len() as u64,
            next_seq: 1,
            inflight: None,
            unresolved: HashMap::new(),
            keys: HashMap::new(),
            written_gen: 0,
            synced_gen: 0,
            dirty_since: None,
            broken: None,
            recs: 0,
            bytes: 0,
            quarantine: Quarantine::default(),
            closed: false,
        };
        Ok(Self::wrap(fs, clock, opts, dir, state))
    }

    /// Reopen an existing recovery dir after a crash/relaunch. Recovers, physically cuts any
    /// torn tail, and continues appending. The returned `Recovery` says what to replay and
    /// which intents are unresolved; the host must `resolve_unfinished` those before `begin`.
    pub fn open(
        root: &RecoveryRoot,
        fs: Arc<dyn Fs>,
        doc_id: &str,
        opts: Options,
        clock: Arc<dyn Clock>,
    ) -> Result<(Journal, Recovery)> {
        let dir = root.doc_dir(doc_id)?;
        let _ = fs.exclude_from_backup(root.path());
        let rec = recover(fs.as_ref(), &dir)?;
        let log = dir.join(LOG_FILE);
        if rec.valid_len < MAGIC.len() as u64 || rec.clean {
            // Missing/torn header, or a clean marker: start a fresh log.
            fs.write_atomic(&log, &header_file())?;
        } else if rec.discarded_bytes > 0 {
            fs.truncate(&log, rec.valid_len)?;
        }
        let good_len = if rec.valid_len < MAGIC.len() as u64 || rec.clean {
            MAGIC.len() as u64
        } else {
            rec.valid_len
        };
        let fresh = rec.clean;
        let file: Arc<dyn FsFile> = fs.open_append(&log)?.into();
        let quarantine = Quarantine::load(fs.as_ref(), &dir);
        let mut unresolved = HashMap::new();
        for u in &rec.unfinished {
            unresolved.insert(
                u.seq,
                (
                    u.command.clone(),
                    command_key(&u.command, &u.params, &u.blobs),
                ),
            );
        }
        let state = State {
            file,
            good_len,
            next_seq: rec.last_seq + 1,
            inflight: None,
            unresolved: unresolved.clone(),
            keys: unresolved,
            written_gen: 0,
            synced_gen: 0,
            dirty_since: None,
            broken: None,
            recs: if fresh { 0 } else { rec.record_count },
            bytes: if fresh { 0 } else { good_len },
            quarantine,
            closed: false,
        };
        let j = Self::wrap(fs, clock, opts, dir, state);
        j.gc_blobs(&rec);
        Ok((j, rec))
    }

    fn wrap(
        fs: Arc<dyn Fs>,
        clock: Arc<dyn Clock>,
        opts: Options,
        dir: PathBuf,
        state: State,
    ) -> Journal {
        Journal {
            inner: Arc::new(Inner {
                fs,
                clock,
                opts,
                dir,
                state: Mutex::new(state),
            }),
        }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.inner.dir
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn live<'a>(&self, s: &'a mut State) -> Result<&'a mut State> {
        if s.closed {
            return Err(JournalError::Closed);
        }
        if let Some(m) = &s.broken {
            return Err(JournalError::Broken(m.clone()));
        }
        Ok(s)
    }

    /// Store an external input (inserted PDF, image...) as `blobs/<blake3>`. Durable on return,
    /// so an Intent that references it can never outlive it.
    pub fn put_blob(&self, data: &[u8]) -> Result<BlobRef> {
        let hash: Hash = *blake3::hash(data).as_bytes();
        let path = blob_path(&self.inner.dir, &hash);
        if self
            .inner
            .fs
            .stat(&path)
            .map(|s| s.len == data.len() as u64)
            .unwrap_or(false)
        {
            return Ok(BlobRef {
                hash,
                len: data.len() as u64,
            });
        }
        self.inner
            .fs
            .create_dir_all(&self.inner.dir.join(BLOB_DIR))?;
        self.inner.fs.write_atomic(&path, data)?;
        Ok(BlobRef {
            hash,
            len: data.len() as u64,
        })
    }

    pub fn read_blob(&self, b: &BlobRef) -> Result<Vec<u8>> {
        let data = self.inner.fs.read(&blob_path(&self.inner.dir, &b.hash))?;
        if blake3::hash(&data).as_bytes() != &b.hash {
            return Err(JournalError::Corrupt(format!(
                "blob {} fails its hash",
                hash_hex(&b.hash)
            )));
        }
        Ok(data)
    }

    /// Is this exact command quarantined (crashed the engine twice)?
    pub fn is_quarantined(
        &self,
        command: &str,
        params: &serde_json::Value,
        blobs: &[BlobRef],
    ) -> bool {
        self.lock()
            .quarantine
            .crashes(&command_key(command, params, blobs))
            >= CRASH_LIMIT
    }

    /// Append `rec`. Caller holds the lock. Rolls the file back to the last good length on a
    /// failed write so a torn record never sits in the middle of the log.
    fn append(&self, s: &mut State, rec: &Record) -> Result<usize> {
        let bytes = encode(rec, self.inner.opts.comp())?;
        if let Err(e) = s.file.append(&bytes) {
            let rolled = self
                .inner
                .fs
                .truncate(&self.inner.dir.join(LOG_FILE), s.good_len);
            if rolled.is_err() {
                s.broken = Some(format!("append failed ({e}) and rollback failed"));
            }
            return Err(e.into());
        }
        s.good_len += bytes.len() as u64;
        s.written_gen += 1;
        s.recs += 1;
        s.bytes += bytes.len() as u64;
        s.dirty_since.get_or_insert(self.inner.clock.now_ms());
        Ok(bytes.len())
    }

    /// Write the `Intent` record. Must complete before the engine sees the command; use
    /// `Dispatcher` to make that structural. The write is a plain append (page cache), so a
    /// host/engine/renderer crash after this returns cannot lose it.
    pub fn begin(
        &self,
        command: &str,
        params: serde_json::Value,
        blobs: Vec<BlobRef>,
    ) -> Result<IntentToken> {
        let key = command_key(command, &params, &blobs);
        let (seq, large) = {
            let mut g = self.lock();
            let s = self.live(&mut g)?;
            let crashes = s.quarantine.crashes(&key);
            if crashes >= CRASH_LIMIT {
                return Err(JournalError::Quarantined {
                    command: command.into(),
                    crashes,
                });
            }
            if let Some(&seq) = s.unresolved.keys().min() {
                return Err(JournalError::Unresolved { seq });
            }
            if s.inflight.is_some() {
                return Err(JournalError::Busy);
            }
            let seq = s.next_seq;
            let big_blob = blobs
                .iter()
                .any(|b| b.len as usize > self.inner.opts.large_payload_bytes);
            let rec = Record::Intent {
                seq,
                ts_ms: self.inner.clock.now_ms(),
                command: command.into(),
                params,
                blobs,
            };
            let n = self.append(s, &rec)?;
            s.next_seq += 1;
            s.inflight = Some(seq);
            s.keys.insert(seq, (command.into(), key));
            (seq, big_blob || n > self.inner.opts.large_payload_bytes)
        };
        if large {
            self.sync_now()?;
        }
        Ok(IntentToken { seq })
    }

    /// Write the `Commit` with after-images once the engine returned its ChangeSet.
    pub fn commit(
        &self,
        token: IntentToken,
        after_images: Vec<AfterImage>,
        created: Vec<ObjId>,
    ) -> Result<()> {
        let large = {
            let mut g = self.lock();
            let s = self.live(&mut g)?;
            if s.inflight != Some(token.seq) {
                return Err(JournalError::NotInFlight);
            }
            let digest = commit_digest(&after_images, &created);
            let rec = Record::Commit {
                seq: token.seq,
                ts_ms: self.inner.clock.now_ms(),
                after_images,
                created,
                digest,
            };
            let n = self.append(s, &rec)?;
            s.inflight = None;
            s.keys.remove(&token.seq);
            n > self.inner.opts.large_payload_bytes
        };
        if large {
            self.sync_now()?;
        }
        Ok(())
    }

    /// The engine returned a clean error (nothing applied): close the intent without a crash.
    pub fn fail(&self, token: IntentToken) -> Result<()> {
        let mut g = self.lock();
        let s = self.live(&mut g)?;
        if s.inflight != Some(token.seq) {
            return Err(JournalError::NotInFlight);
        }
        let rec = Record::Abandon {
            seq: token.seq,
            ts_ms: self.inner.clock.now_ms(),
            crashed: false,
        };
        self.append(s, &rec)?;
        s.inflight = None;
        s.keys.remove(&token.seq);
        Ok(())
    }

    /// The in-flight command (or a recovered unfinished one) crashed the engine/host:
    /// count the crash towards quarantine and record the resolution. Idempotent per seq.
    /// Returns the crash count for this command.
    pub fn resolve_unfinished(&self, seq: u64, _how: Resolution) -> Result<u32> {
        let mut g = self.lock();
        let s = self.live(&mut g)?;
        let (command, key) = s
            .unresolved
            .get(&seq)
            .cloned()
            .or_else(|| {
                (s.inflight == Some(seq))
                    .then(|| s.keys.get(&seq).cloned())
                    .flatten()
            })
            .ok_or(JournalError::NotInFlight)?;
        // Counter first (durable), then the Abandon: a crash in between is re-counted as a
        // no-op because the seq is remembered.
        if s.quarantine.bump(&key, &command, seq) {
            s.quarantine.save(self.inner.fs.as_ref(), &self.inner.dir)?;
        }
        let rec = Record::Abandon {
            seq,
            ts_ms: self.inner.clock.now_ms(),
            crashed: true,
        };
        self.append(s, &rec)?;
        s.unresolved.remove(&seq);
        if s.inflight == Some(seq) {
            s.inflight = None;
        }
        s.keys.remove(&seq);
        Ok(s.quarantine.crashes(&key))
    }

    /// Unflushed state, for tests and diagnostics.
    pub fn unsynced(&self) -> bool {
        let g = self.lock();
        g.synced_gen < g.written_gen
    }

    /// fsync now (blocks the caller).
    pub fn sync_now(&self) -> Result<()> {
        let (file, upto) = {
            let mut g = self.lock();
            let s = self.live(&mut g)?;
            if s.synced_gen >= s.written_gen {
                return Ok(());
            }
            (s.file.clone(), s.written_gen)
        };
        // The lock is not held across the fsync: appends continue meanwhile.
        let r = file.sync();
        let mut g = self.lock();
        match r {
            Ok(()) => {
                g.synced_gen = g.synced_gen.max(upto);
                g.dirty_since = (g.synced_gen < g.written_gen).then(|| self.inner.clock.now_ms());
                Ok(())
            }
            Err(e) => {
                g.broken = Some(format!("fsync failed: {e}"));
                Err(e.into())
            }
        }
    }

    /// Group commit: fsync only if the oldest unsynced record is older than the interval.
    /// Called by the flusher thread (or manually with a manual clock in tests).
    pub fn sync_if_due(&self) -> Result<bool> {
        let due = {
            let g = self.lock();
            match g.dirty_since {
                Some(t) if g.synced_gen < g.written_gen => {
                    self.inner.clock.now_ms().saturating_sub(t) >= self.inner.opts.interval_ms()
                }
                _ => false,
            }
        };
        if due {
            self.sync_now()?;
        }
        Ok(due)
    }

    /// Spawn the background group-fsync thread. Dropping the handle stops it.
    pub fn spawn_flusher(&self) -> Flusher {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let tick = Duration::from_millis((self.inner.opts.interval_ms() / 4).clamp(25, 250));
        let handle = std::thread::Builder::new()
            .name("papyrine-journal-flush".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    let Some(inner) = weak.upgrade() else { break };
                    let _ = Journal { inner }.sync_if_due();
                }
            })
            .expect("spawn flusher");
        Flusher {
            stop,
            handle: Some(handle),
        }
    }

    /// True when the log passed 32 MB or 500 records.
    pub fn checkpoint_due(&self) -> bool {
        let g = self.lock();
        g.recs >= self.inner.opts.checkpoint_records || g.bytes >= self.inner.opts.checkpoint_bytes
    }

    /// Persist an engine-produced ID-preserving checkpoint and truncate the journal to a
    /// single Checkpoint record. Requires a quiescent journal (nothing in flight or unresolved).
    ///
    /// Crash safety: the blob is content-addressed and durable first; the log swap is an atomic
    /// rename; until it lands the old log (which the checkpoint merely duplicates) stays valid.
    pub fn checkpoint(&self, state_bytes: &[u8]) -> Result<()> {
        self.sync_now()?;
        let mut g = self.lock();
        let s = self.live(&mut g)?;
        if s.inflight.is_some() {
            return Err(JournalError::Busy);
        }
        if let Some(&seq) = s.unresolved.keys().min() {
            return Err(JournalError::Unresolved { seq });
        }
        let hash: Hash = *blake3::hash(state_bytes).as_bytes();
        let blob = BlobRef {
            hash,
            len: state_bytes.len() as u64,
        };
        self.inner
            .fs
            .create_dir_all(&self.inner.dir.join(BLOB_DIR))?;
        self.inner
            .fs
            .write_atomic(&blob_path(&self.inner.dir, &hash), state_bytes)?;
        let rec = Record::Checkpoint {
            upto_seq: s.next_seq - 1,
            ts_ms: self.inner.clock.now_ms(),
            blob,
        };
        let mut image = header_file();
        image.extend_from_slice(&encode(&rec, self.inner.opts.comp())?);
        self.inner
            .fs
            .write_atomic(&self.inner.dir.join(LOG_FILE), &image)?;
        s.file = self
            .inner
            .fs
            .open_append(&self.inner.dir.join(LOG_FILE))?
            .into();
        s.good_len = image.len() as u64;
        s.recs = 1;
        s.bytes = image.len() as u64;
        s.written_gen += 1;
        s.synced_gen = s.written_gen;
        s.dirty_since = None;
        drop(g);
        self.gc_keep(&[hash]);
        Ok(())
    }

    /// Document saved: the saved file is the new base. Writes a Clean marker, replaces
    /// base.json, empties the log and drops blobs. The recovery dir stays for further edits.
    pub fn rebase(&self, new_base: &BaseInfo) -> Result<()> {
        let mut g = self.lock();
        let s = self.live(&mut g)?;
        if s.inflight.is_some() {
            return Err(JournalError::Busy);
        }
        let clean = Record::Clean {
            ts_ms: self.inner.clock.now_ms(),
        };
        self.append(s, &clean)?;
        s.file.sync()?;
        self.inner.fs.write_atomic(
            &self.inner.dir.join(BASE_FILE),
            &serde_json::to_vec_pretty(new_base)?,
        )?;
        self.inner
            .fs
            .write_atomic(&self.inner.dir.join(LOG_FILE), &header_file())?;
        s.file = self
            .inner
            .fs
            .open_append(&self.inner.dir.join(LOG_FILE))?
            .into();
        s.good_len = MAGIC.len() as u64;
        s.recs = 0;
        s.bytes = 0;
        s.written_gen += 1;
        s.synced_gen = s.written_gen;
        s.dirty_since = None;
        s.unresolved.clear();
        drop(g);
        self.gc_keep(&[]);
        Ok(())
    }

    /// Clean save-and-close, or close with "Don't save": delete the recovery dir.
    pub fn discard(&self) -> Result<()> {
        let mut g = self.lock();
        g.closed = true;
        let clean = Record::Clean {
            ts_ms: self.inner.clock.now_ms(),
        };
        // Best effort marker first, in case removal is interrupted half-way.
        if let Ok(b) = encode(&clean, self.inner.opts.comp()) {
            let _ = g.file.append(&b);
            let _ = g.file.sync();
        }
        drop(g);
        match self.inner.fs.remove_dir_all(&self.inner.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Stop accepting records after a final fsync, keeping the dir (crash-like exit path).
    pub fn close(&self) -> Result<()> {
        let r = self.sync_now();
        self.lock().closed = true;
        r
    }

    fn gc_keep(&self, keep: &[Hash]) {
        let dir = self.inner.dir.join(BLOB_DIR);
        let keep: BTreeSet<String> = keep.iter().map(hash_hex).collect();
        for e in self.inner.fs.list_dir(&dir).unwrap_or_default() {
            if !keep.contains(&e.name) {
                let _ = self.inner.fs.remove_file(&e.path);
            }
        }
    }

    /// Remove blobs no surviving record references (leftovers of an interrupted checkpoint).
    fn gc_blobs(&self, rec: &Recovery) {
        let mut keep: Vec<Hash> = Vec::new();
        if let Some(c) = &rec.checkpoint {
            keep.push(c.blob.hash);
        }
        for c in &rec.commits {
            keep.extend(c.blobs.iter().map(|b| b.hash));
        }
        for u in &rec.unfinished {
            keep.extend(u.blobs.iter().map(|b| b.hash));
        }
        // Only collect when nothing is unresolved, so a pending Redo keeps its inputs.
        if self.lock().unresolved.is_empty() {
            self.gc_keep(&keep);
        }
    }
}

/// Handle for the background fsync thread.
pub struct Flusher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Flusher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Final best-effort flush when the last handle goes away.
        if let Ok(s) = self.state.lock()
            && !s.closed
            && s.synced_gen < s.written_gen
        {
            let _ = s.file.sync();
        }
    }
}
