//! papyrine-sandbox: per-OS sandboxes for the engine, renderer and component
//! helper processes (docs/ARCHITECTURE.md §8, ROADMAP 1.9).
//!
//! The host is the only process with filesystem, dialog and network access.
//! A child calls [`apply_child_sandbox`] as early as possible (before it opens
//! any file it was not handed, and before it starts threads) and from then on:
//!
//! * cannot read anything outside the [`Profile`] allowlist (not `~`),
//! * cannot write anywhere except the per-child temp directory,
//! * cannot create sockets or reach the network, and
//! * cannot start another process.
//!
//! | OS | Mechanism |
//! |---|---|
//! | macOS | Seatbelt profile via `sandbox_init`, applied inside the child |
//! | Linux | `PR_SET_NO_NEW_PRIVS` + Landlock filesystem allowlist + seccomp-bpf filter, applied inside the child |
//! | Windows | restrictions are fixed at `CreateProcess`, so the **parent** must start the child with [`windows::spawn_restricted`] (restricted token, untrusted integrity level, Job object with a one-process cap and a memory cap); [`apply_child_sandbox`] then verifies that and adds process mitigation policies |
//!
//! Files cross the boundary as already-open handles or as bytes (see
//! `papyrine-ipc`), never as paths the child opens by itself.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub mod windows;

mod tempdir;
pub use tempdir::ChildTempDir;

/// What a sandboxed child may touch. Paths are canonicalised by
/// [`Profile::normalized`] (macOS Seatbelt matches real paths, so `/var/...`
/// must be `/private/var/...`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Directories readable recursively: the app bundle / executable dir, the
    /// component directory, the PDFium directory, font directories ...
    pub read_dirs: Vec<PathBuf>,
    /// Individual files readable.
    pub read_files: Vec<PathBuf>,
    /// The per-child temporary directory: the only place the child may write.
    pub temp_dir: PathBuf,
    /// Address-space/commit cap in bytes (Windows Job object; Unix `RLIMIT_AS`
    /// is not used because mmap-heavy PDF workloads reserve a lot of address
    /// space). `None` = unlimited.
    pub memory_limit: Option<u64>,
    /// When true (default for [`Profile::new`]) a sandbox that cannot be fully
    /// enforced (for example Landlock missing from the kernel) is an error
    /// instead of a silent downgrade.
    pub require_enforced: bool,
}

impl Profile {
    pub fn new(temp_dir: impl Into<PathBuf>) -> Self {
        Self {
            temp_dir: temp_dir.into(),
            require_enforced: true,
            ..Self::default()
        }
    }

    pub fn read_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.read_dirs.push(p.into());
        self
    }

    pub fn read_file(mut self, p: impl Into<PathBuf>) -> Self {
        self.read_files.push(p.into());
        self
    }

    pub fn memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit = Some(bytes);
        self
    }

    /// Add the directory of the running executable (the app bundle contents).
    pub fn with_exe_dir(mut self) -> Self {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                self.read_dirs.push(dir.to_path_buf());
            }
            // Test binaries live in `deps/` and load siblings from the parent.
            #[cfg(any(test, debug_assertions))]
            if let Some(dir) = exe.parent().and_then(Path::parent) {
                self.read_dirs.push(dir.to_path_buf());
            }
        }
        self
    }

    /// Canonicalise every path; missing allowlist entries are dropped (they
    /// grant nothing), a missing temp dir is an error.
    pub fn normalized(&self) -> Result<Profile> {
        let canon = |p: &PathBuf| std::fs::canonicalize(p).ok().map(strip_verbatim);
        let temp = std::fs::canonicalize(&self.temp_dir)
            .map(strip_verbatim)
            .map_err(|e| Error::Setup(format!("temp dir {}: {e}", self.temp_dir.display())))?;
        let mut read_dirs: Vec<PathBuf> = self.read_dirs.iter().filter_map(canon).collect();
        read_dirs.sort();
        read_dirs.dedup();
        let mut read_files: Vec<PathBuf> = self.read_files.iter().filter_map(canon).collect();
        read_files.sort();
        read_files.dedup();
        Ok(Profile {
            read_dirs,
            read_files,
            temp_dir: temp,
            memory_limit: self.memory_limit,
            require_enforced: self.require_enforced,
        })
    }
}

/// `\\?\C:\x` -> `C:\x`; a no-op elsewhere.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest);
        }
    }
    p
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sandbox setup failed: {0}")]
    Setup(String),
    /// The OS could not enforce the full profile and `require_enforced` is set.
    #[error("sandbox not fully enforced: {0}")]
    NotEnforced(String),
    #[error("unsupported platform: {0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What [`apply_child_sandbox`] actually enforced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Short names of the mechanisms that are now active.
    pub mechanisms: Vec<String>,
    /// Anything requested but not enforced (only possible when
    /// `require_enforced` is false).
    pub degraded: Vec<String>,
}

/// Lock the calling process down. Irreversible. Call it before starting
/// threads (Landlock and Seatbelt are inherited by new threads; on Linux the
/// seccomp filter is synchronised to existing threads, Landlock is not).
pub fn apply_child_sandbox(profile: &Profile) -> Result<Report> {
    let p = profile.normalized()?;
    #[cfg(target_os = "macos")]
    return macos::apply(&p);
    #[cfg(target_os = "linux")]
    return linux::apply(&p);
    #[cfg(windows)]
    return windows::apply(&p);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = p;
        Err(Error::Unsupported("no sandbox for this OS"))
    }
}

/// Apply the OS defaults that make a child's temp directory the one place it
/// can create files: set `TMPDIR`/`TEMP`/`TMP`. Call this in the parent on the
/// child's `Command` environment (done by `papyrine-ipc`'s spawn helper).
pub fn temp_env(temp_dir: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("TMPDIR", temp_dir.to_path_buf()),
        ("TEMP", temp_dir.to_path_buf()),
        ("TMP", temp_dir.to_path_buf()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_is_unique_and_removed_on_drop() {
        let a = ChildTempDir::create().unwrap();
        let b = ChildTempDir::create().unwrap();
        assert_ne!(a.path(), b.path());
        let p = a.path().to_path_buf();
        std::fs::write(p.join("f"), b"x").unwrap();
        drop(a);
        assert!(!p.exists());
    }

    #[test]
    fn normalized_drops_missing_entries_and_canonicalises() {
        let t = ChildTempDir::create().unwrap();
        let p = Profile::new(t.path())
            .read_dir(t.path().join("missing"))
            .read_dir(t.path())
            .read_dir(t.path().join("."));
        let n = p.normalized().unwrap();
        assert_eq!(n.read_dirs.len(), 1, "{:?}", n.read_dirs);
        assert!(n.require_enforced);
        assert!(Profile::new(t.path().join("nope")).normalized().is_err());
    }
}
