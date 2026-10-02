//! Test support: an in-memory filesystem that distinguishes written from fsynced data and
//! can drop the former (simulated power loss) or fail writes.

use crate::fs::{DirEntry, Fs, FsFile, Stat};
use crate::journal::Clock;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Node {
    durable: Vec<u8>,
    volatile: Vec<u8>,
    mtime_ms: u64,
}

#[derive(Default)]
struct St {
    files: BTreeMap<PathBuf, Node>,
    dirs: BTreeMap<PathBuf, u64>,
    now_ms: u64,
    /// Next append writes half its bytes then fails.
    torn_append: bool,
    fail_sync: bool,
    fail_truncate: bool,
    syncs: u64,
    excluded: Vec<PathBuf>,
}

/// How much unsynced data survives a power loss.
#[derive(Debug, Clone, Copy)]
pub enum PowerLoss {
    /// Drop everything that was not fsynced.
    DropUnsynced,
    /// Keep a pseudo-random prefix of each file's unsynced tail (tears records mid-way).
    KeepPrefix(u64),
}

#[derive(Clone, Default)]
pub struct FaultFs(Arc<Mutex<St>>);

impl FaultFs {
    pub fn new() -> Self {
        Self::default()
    }
    fn st(&self) -> std::sync::MutexGuard<'_, St> {
        self.0.lock().unwrap()
    }
    pub fn set_now(&self, ms: u64) {
        self.st().now_ms = ms;
    }
    pub fn power_loss(&self, mode: PowerLoss) {
        let mut s = self.st();
        for n in s.files.values_mut() {
            let keep = match mode {
                PowerLoss::DropUnsynced => 0,
                PowerLoss::KeepPrefix(seed) => {
                    let extra = n.volatile.len().saturating_sub(n.durable.len());
                    if extra == 0 {
                        0
                    } else {
                        (seed.wrapping_mul(0x9E3779B97F4A7C15) >> 11) as usize % (extra + 1)
                    }
                }
            };
            n.volatile.truncate(n.durable.len() + keep);
            n.durable = n.volatile.clone();
        }
    }
    /// The next `append` writes only half of its bytes and returns an error.
    pub fn fail_next_append_torn(&self) {
        self.st().torn_append = true;
    }
    pub fn set_fail_sync(&self, v: bool) {
        self.st().fail_sync = v;
    }
    pub fn set_fail_truncate(&self, v: bool) {
        self.st().fail_truncate = v;
    }
    pub fn sync_count(&self) -> u64 {
        self.st().syncs
    }
    pub fn excluded(&self) -> Vec<PathBuf> {
        self.st().excluded.clone()
    }
    pub fn file_len(&self, p: &Path) -> Option<usize> {
        self.st().files.get(p).map(|n| n.volatile.len())
    }
    pub fn corrupt_byte(&self, p: &Path, at: usize) {
        if let Some(n) = self.st().files.get_mut(p) {
            n.volatile[at] ^= 0x5A;
            n.durable = n.volatile.clone();
        }
    }
}

struct FaultFile {
    st: Arc<Mutex<St>>,
    path: PathBuf,
}

impl FsFile for FaultFile {
    fn append(&self, buf: &[u8]) -> io::Result<()> {
        let mut s = self.st.lock().unwrap();
        let torn = std::mem::take(&mut s.torn_append);
        let now = s.now_ms;
        let n = s
            .files
            .get_mut(&self.path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        n.mtime_ms = now;
        if torn {
            n.volatile.extend_from_slice(&buf[..buf.len() / 2]);
            return Err(io::Error::other("injected torn append"));
        }
        n.volatile.extend_from_slice(buf);
        Ok(())
    }
    fn sync(&self) -> io::Result<()> {
        let mut s = self.st.lock().unwrap();
        if s.fail_sync {
            return Err(io::Error::other("injected fsync failure"));
        }
        s.syncs += 1;
        if let Some(n) = s.files.get_mut(&self.path) {
            n.durable = n.volatile.clone();
        }
        Ok(())
    }
}

fn nf() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}

impl Fs for FaultFs {
    fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        let mut s = self.st();
        let now = s.now_ms;
        for a in p.ancestors() {
            s.dirs.entry(a.to_path_buf()).or_insert(now);
        }
        Ok(())
    }
    fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        self.st()
            .files
            .get(p)
            .map(|n| n.volatile.clone())
            .ok_or_else(nf)
    }
    fn write_atomic(&self, p: &Path, data: &[u8]) -> io::Result<()> {
        let mut s = self.st();
        let now = s.now_ms;
        if let Some(parent) = p.parent() {
            s.dirs.entry(parent.to_path_buf()).or_insert(now);
        }
        s.files.insert(
            p.to_path_buf(),
            Node {
                durable: data.to_vec(),
                volatile: data.to_vec(),
                mtime_ms: now,
            },
        );
        Ok(())
    }
    fn open_append(&self, p: &Path) -> io::Result<Box<dyn FsFile>> {
        let mut s = self.st();
        let now = s.now_ms;
        s.files.entry(p.to_path_buf()).or_insert_with(|| Node {
            mtime_ms: now,
            ..Default::default()
        });
        Ok(Box::new(FaultFile {
            st: self.0.clone(),
            path: p.to_path_buf(),
        }))
    }
    fn truncate(&self, p: &Path, len: u64) -> io::Result<()> {
        let mut s = self.st();
        if s.fail_truncate {
            return Err(io::Error::other("injected truncate failure"));
        }
        let n = s.files.get_mut(p).ok_or_else(nf)?;
        n.volatile.truncate(len as usize);
        n.durable = n.volatile.clone();
        Ok(())
    }
    fn remove_file(&self, p: &Path) -> io::Result<()> {
        self.st().files.remove(p).map(|_| ()).ok_or_else(nf)
    }
    fn remove_dir_all(&self, p: &Path) -> io::Result<()> {
        let mut s = self.st();
        s.files.retain(|k, _| !k.starts_with(p));
        s.dirs.retain(|k, _| !k.starts_with(p));
        Ok(())
    }
    fn list_dir(&self, p: &Path) -> io::Result<Vec<DirEntry>> {
        let s = self.st();
        let mut out: BTreeMap<String, DirEntry> = BTreeMap::new();
        for k in s.files.keys() {
            if k.parent() == Some(p) {
                let name = k.file_name().unwrap().to_string_lossy().into_owned();
                out.insert(
                    name.clone(),
                    DirEntry {
                        name,
                        path: k.clone(),
                        is_dir: false,
                    },
                );
            }
        }
        for k in s.dirs.keys() {
            if k.parent() == Some(p) {
                let name = k.file_name().unwrap().to_string_lossy().into_owned();
                out.insert(
                    name.clone(),
                    DirEntry {
                        name,
                        path: k.clone(),
                        is_dir: true,
                    },
                );
            }
        }
        Ok(out.into_values().collect())
    }
    fn stat(&self, p: &Path) -> io::Result<Stat> {
        let s = self.st();
        if let Some(n) = s.files.get(p) {
            return Ok(Stat {
                len: n.volatile.len() as u64,
                mtime_ms: n.mtime_ms,
                is_dir: false,
            });
        }
        if let Some(&m) = s.dirs.get(p) {
            return Ok(Stat {
                len: 0,
                mtime_ms: m,
                is_dir: true,
            });
        }
        Err(nf())
    }
    fn exclude_from_backup(&self, p: &Path) -> io::Result<()> {
        self.st().excluded.push(p.to_path_buf());
        Ok(())
    }
}

/// A clock you advance by hand.
#[derive(Clone, Default)]
pub struct ManualClock(Arc<AtomicU64>);
impl ManualClock {
    pub fn new(ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ms)))
    }
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}
impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
