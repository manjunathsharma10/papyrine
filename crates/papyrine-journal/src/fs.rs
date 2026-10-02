//! Filesystem abstraction so tests can inject faults (see `testing::FaultFs`).

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Stat {
    pub len: u64,
    pub mtime_ms: u64,
    pub is_dir: bool,
}

/// An append-only handle. Methods take `&self` so an fsync can run on the flusher thread
/// while the writer keeps appending; callers serialise appends themselves.
pub trait FsFile: Send + Sync {
    /// Append all bytes. After this returns the data is visible to other processes (page
    /// cache), which is what makes it survive a process kill.
    fn append(&self, buf: &[u8]) -> io::Result<()>;
    /// Durable flush (F_FULLFSYNC on macOS).
    fn sync(&self) -> io::Result<()>;
}

pub trait Fs: Send + Sync + 'static {
    /// Create the directory chain with owner-only permissions.
    fn create_dir_all(&self, p: &Path) -> io::Result<()>;
    fn read(&self, p: &Path) -> io::Result<Vec<u8>>;
    /// Write a whole file crash-atomically and durably (tmp + fsync + rename + dir fsync), 0600.
    fn write_atomic(&self, p: &Path, data: &[u8]) -> io::Result<()>;
    /// Open for append, creating (0600) if needed.
    fn open_append(&self, p: &Path) -> io::Result<Box<dyn FsFile>>;
    /// Truncate to `len` and make that durable.
    fn truncate(&self, p: &Path, len: u64) -> io::Result<()>;
    fn remove_file(&self, p: &Path) -> io::Result<()>;
    fn remove_dir_all(&self, p: &Path) -> io::Result<()>;
    fn list_dir(&self, p: &Path) -> io::Result<Vec<DirEntry>>;
    fn stat(&self, p: &Path) -> io::Result<Stat>;
    /// Best effort: keep the path out of OS backups (Time Machine).
    fn exclude_from_backup(&self, p: &Path) -> io::Result<()>;
}

/// The real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

struct RealFile(std::fs::File);

fn full_sync(f: &std::fs::File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl(F_FULLFSYNC) on a valid, owned fd takes no pointers.
        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) } == 0 {
            return Ok(());
        }
        // Some filesystems (network, FAT) reject F_FULLFSYNC; fall back to fsync.
    }
    f.sync_all()
}

impl FsFile for RealFile {
    fn append(&self, buf: &[u8]) -> io::Result<()> {
        use std::io::Write;
        (&self.0).write_all(buf)
    }
    fn sync(&self) -> io::Result<()> {
        full_sync(&self.0)
    }
}

fn sync_dir(p: &Path) {
    #[cfg(unix)]
    if let Some(parent) = p.parent()
        && let Ok(d) = std::fs::File::open(parent)
    {
        let _ = full_sync(&d);
    }
    #[cfg(not(unix))]
    let _ = p;
}

fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

fn ms(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Fs for RealFs {
    fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(p)
    }
    fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(p)
    }
    fn write_atomic(&self, p: &Path, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut tmp = p.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        {
            let mut f = private_options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            f.write_all(data)?;
            full_sync(&f)?;
        }
        std::fs::rename(&tmp, p)?;
        sync_dir(p);
        Ok(())
    }
    fn open_append(&self, p: &Path) -> io::Result<Box<dyn FsFile>> {
        let existed = p.exists();
        let f = private_options().append(true).create(true).open(p)?;
        if !existed {
            sync_dir(p);
        }
        Ok(Box::new(RealFile(f)))
    }
    fn truncate(&self, p: &Path, len: u64) -> io::Result<()> {
        let f = std::fs::OpenOptions::new().write(true).open(p)?;
        f.set_len(len)?;
        full_sync(&f)
    }
    fn remove_file(&self, p: &Path) -> io::Result<()> {
        std::fs::remove_file(p)
    }
    fn remove_dir_all(&self, p: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(p)
    }
    fn list_dir(&self, p: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(p)? {
            let e = e?;
            out.push(DirEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir: e.file_type()?.is_dir(),
            });
        }
        Ok(out)
    }
    fn stat(&self, p: &Path) -> io::Result<Stat> {
        let m = std::fs::metadata(p)?;
        Ok(Stat {
            len: m.len(),
            mtime_ms: m.modified().map(ms).unwrap_or(0),
            is_dir: m.is_dir(),
        })
    }
    fn exclude_from_backup(&self, p: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            use std::ffi::{CString, c_void};
            use std::os::unix::ffi::OsStrExt;
            let path = CString::new(p.as_os_str().as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            let name = c"com.apple.metadata:com_apple_backup_excludeItem";
            let val = backup_exclude_plist();
            // SAFETY: all pointers are valid for the stated lengths for the call's duration.
            let rc = unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    val.as_ptr().cast::<c_void>(),
                    val.len(),
                    0,
                    0,
                )
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = p;
        Ok(())
    }
}

/// Binary plist holding the single string "com.apple.backupd", the value Time Machine
/// looks for in `com.apple.metadata:com_apple_backup_excludeItem` (the same sticky
/// exclusion `tmutil addexclusion` sets).
pub fn backup_exclude_plist() -> Vec<u8> {
    const S: &[u8] = b"com.apple.backupd";
    let mut v = Vec::with_capacity(61);
    v.extend_from_slice(b"bplist00");
    v.extend_from_slice(&[0x5F, 0x10, S.len() as u8]); // ASCII string, length in a 1-byte int
    v.extend_from_slice(S);
    let table_off = v.len() as u64; // 28
    v.push(8); // offset of object 0
    v.extend_from_slice(&[0; 6]); // trailer: unused
    v.push(1); // offset int size
    v.push(1); // object ref size
    v.extend_from_slice(&1u64.to_be_bytes()); // object count
    v.extend_from_slice(&0u64.to_be_bytes()); // top object
    v.extend_from_slice(&table_off.to_be_bytes()); // offset table offset
    v
}
