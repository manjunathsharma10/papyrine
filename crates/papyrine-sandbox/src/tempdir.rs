//! The per-child temporary directory: created by the host with a random name,
//! removed (recursively) when the host drops it.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct ChildTempDir {
    path: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl ChildTempDir {
    /// Create `<parent>/papyrine-<pid>-<n>-<random>` (mode 0700 on Unix).
    pub fn create_in(parent: &Path) -> io::Result<Self> {
        for _ in 0..16 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let r = random_u64();
            let path = parent.join(format!("papyrine-{}-{n}-{r:016x}", std::process::id()));
            match create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("could not create a unique temp dir"))
    }

    pub fn create() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ChildTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn create_dir(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(p)
}

#[cfg(windows)]
fn create_dir(p: &Path) -> io::Result<()> {
    crate::windows::create_child_dir(p)
}

#[cfg(not(any(unix, windows)))]
fn create_dir(p: &Path) -> io::Result<()> {
    std::fs::create_dir(p)
}

fn random_u64() -> u64 {
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut b = [0u8; 8];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom")
            && f.read_exact(&mut b).is_ok()
        {
            return u64::from_le_bytes(b);
        }
    }
    // Fallback: time + address entropy; uniqueness is what matters, the dir is 0700 / ACL'd.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let x = &t as *const u64 as u64;
    t ^ x.rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15
}
