//! Small helpers shared by the broker modules.

use std::path::Path;
use std::time::SystemTime;

use papyrine_ipc::{DocSource, Handle, SharedRegion};

use crate::error::{HostErr, Result};

/// A second mapping of the same pages, for sending to a child while keeping ours.
pub fn dup_region(r: &SharedRegion) -> Result<SharedRegion> {
    let h = r.try_clone_handle()?;
    Ok(SharedRegion::from_handle(h, r.len())?)
}

/// A snapshot section the host owns (and the renderer maps read-only).
pub struct Section {
    pub region: SharedRegion,
    pub len: u64,
}

impl Section {
    pub fn source(&self) -> Result<DocSource> {
        Ok(DocSource::Shared {
            region: dup_region(&self.region)?,
            len: self.len,
        })
    }
}

/// The document's file as handed to children: an open read-only handle.
pub struct BaseFile {
    file: std::fs::File,
    pub len: u64,
}

impl BaseFile {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }

    pub fn source(&self) -> Result<DocSource> {
        let f = self.file.try_clone()?;
        Ok(DocSource::File {
            handle: Handle::from_file(f),
            len: self.len,
        })
    }

    /// Positional read (the handle is shared with children, so no cursor is used).
    pub fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, off)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, off)
        }
    }

    /// Copy the first `len` bytes into `out`; `Err` if the file is shorter.
    pub fn copy_prefix(&self, len: u64, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let mut buf = vec![0u8; 1 << 20];
        let mut off = 0u64;
        while off < len {
            let want = buf.len().min((len - off) as usize);
            let n = self.read_at(off, &mut buf[..want])?;
            if n == 0 {
                return Err(std::io::Error::other(
                    "the document's file is shorter than the engine expects",
                ));
            }
            out.write_all(&buf[..n])?;
            off += n as u64;
        }
        Ok(())
    }

    /// "%PDF-1.7" header version, if present in the first KiB.
    pub fn pdf_version(&self) -> String {
        let mut buf = [0u8; 1024];
        let n = self.read_at(0, &mut buf).unwrap_or(0);
        let hay = &buf[..n];
        hay.windows(5)
            .position(|w| w == b"%PDF-")
            .map(|i| {
                hay[i + 5..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit() || **b == b'.')
                    .map(|b| *b as char)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// (mtime ms, size) of a file, for external-change detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub mtime_ms: u64,
    pub size: u64,
}

impl FileStat {
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Some(Self {
            mtime_ms,
            size: m.len(),
        })
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Random lowercase hex id of `bytes` bytes.
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if getrandom::fill(&mut b).is_err() {
        // Extremely unlikely; fall back to time + pid so ids stay unique per process.
        let t = now_ms().to_le_bytes();
        for (i, x) in b.iter_mut().enumerate() {
            *x = t[i % 8] ^ (std::process::id() as u8).wrapping_add(i as u8);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Size a shared output region for a PDF of about `base` bytes plus `extra`.
pub fn out_size(base: u64, extra: u64) -> usize {
    (base + extra + (4 << 20)).min(usize::MAX as u64 / 2) as usize
}

pub fn parse_needed(message: &str) -> Option<usize> {
    message
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

/// `p` canonicalised, if it is inside `dir` (the engine is untrusted: it may only hand the
/// host files from its own temp directory).
pub fn within(dir: &Path, p: &Path) -> Option<std::path::PathBuf> {
    let d = std::fs::canonicalize(dir).ok()?;
    let c = std::fs::canonicalize(p).ok()?;
    (c.starts_with(&d) && c.is_file()).then_some(c)
}

pub fn invalid(m: impl Into<String>) -> HostErr {
    HostErr::internal(m)
}
