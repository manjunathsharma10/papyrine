//! Atomic replace with validation (ARCHITECTURE section 4.5).
//!
//! Steps, in order:
//!
//! * temp file in the same directory;
//! * produce the content;
//! * fsync (`F_FULLFSYNC` on macOS);
//! * validate: qpdf re-parse, exact-prefix check, host-supplied renderer check;
//! * carry over permissions, xattrs and ACLs;
//! * rename (`ReplaceFileW` on Windows);
//! * fsync the directory.
//!
//! The target is only touched by the rename, so a failure or a crash at any earlier step leaves
//! it as it was. After the rename it is the complete new file. [`FaultInjector`] lets tests fail
//! or kill the process before every step.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use papyrine_cos::{Document, OpenOptions, Secret};

use crate::fsutil;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    CreateTemp,
    Fill,
    SyncTemp,
    Validate,
    CopyMetadata,
    Rename,
    SyncDir,
}

impl Step {
    pub const ALL: [Step; 7] = [
        Step::CreateTemp,
        Step::Fill,
        Step::SyncTemp,
        Step::Validate,
        Step::CopyMetadata,
        Step::Rename,
        Step::SyncDir,
    ];
}

/// Called before each step. Return an error to fail the step, or abort the process to simulate
/// a crash.
pub trait FaultInjector {
    fn before(&mut self, step: Step) -> io::Result<()>;
}

/// Host-provided validation: open the file in the renderer and render page 1.
pub type RendererCheck<'a> = &'a dyn Fn(&Path) -> std::result::Result<(), String>;

/// What the new file must satisfy before it may replace the target.
#[derive(Default)]
pub struct Validation<'a> {
    /// Password to open an encrypted result.
    pub password: Option<Secret>,
    pub expect_pages: Option<usize>,
    /// The first `len` bytes must equal the first `len` bytes of `path` (incremental saves).
    pub prefix_of: Option<(&'a Path, u64)>,
    /// Accept a file qpdf had to repair (full rewrites of damaged input only).
    pub allow_repairs: bool,
    /// Host callback: open the file in the renderer and render page 1.
    pub renderer: Option<RendererCheck<'a>>,
}

#[derive(Default)]
pub struct ReplaceOptions<'a> {
    pub validation: Validation<'a>,
    pub faults: Option<&'a mut dyn FaultInjector>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaceOutcome {
    /// False when extended attributes or ACLs of the old file could not be carried over (the
    /// filesystem refused them). Permissions are always carried over or the save fails.
    pub metadata_preserved: bool,
    /// False when the directory fsync failed after the (complete) rename: the new file is in
    /// place but its durability across power loss is not confirmed.
    pub dir_synced: bool,
}

fn temp_path(target: &Path) -> Result<PathBuf> {
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target.file_name().ok_or_else(|| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target has no file name",
        ))
    })?;
    let mut rnd = [0u8; 4];
    getrandom::fill(&mut rnd).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    let mut n = std::ffi::OsString::from(".");
    n.push(name);
    n.push(format!(
        ".{}-{:08x}.papyrine-tmp",
        std::process::id(),
        u32::from_le_bytes(rnd)
    ));
    Ok(dir.join(n))
}

/// Remove temp files a crashed save left next to `target`.
pub fn cleanup_stale_temps(target: &Path) -> io::Result<usize> {
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Some(name) = target.file_name() else {
        return Ok(0);
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let mut n = 0;
    for e in fs::read_dir(dir)? {
        let e = e?;
        let f = e.file_name().to_string_lossy().into_owned();
        if f.starts_with(&prefix)
            && f.ends_with(".papyrine-tmp")
            && fs::remove_file(e.path()).is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

fn same_prefix(a: &Path, b: &Path, len: u64) -> io::Result<bool> {
    let (mut fa, mut fb) = (File::open(a)?.take(len), File::open(b)?.take(len));
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let mut total = 0u64;
    loop {
        let na = read_full(&mut fa, &mut ba)?;
        let nb = read_full(&mut fb, &mut bb)?;
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        total += na as u64;
        if na == 0 {
            return Ok(total == len);
        }
    }
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Does the repair log say qpdf had to rebuild the cross-reference data?
pub fn xref_was_repaired(log: &papyrine_cos::RepairLog) -> bool {
    [
        "reconstruct",
        "recovering damaged file",
        "file is damaged",
        "can't find startxref",
        "xref table not found",
    ]
    .iter()
    .any(|n| log.mentions(n))
}

/// Validate `path` against `v`; used before the rename and exposed for hosts.
pub fn validate_file(path: &Path, v: &Validation<'_>) -> Result<()> {
    let fail = |m: String| Error::Validation(m);
    if let Some((base, len)) = v.prefix_of
        && !same_prefix(path, base, len)?
    {
        return Err(fail(
            "original bytes are not an exact prefix of the new file".into(),
        ));
    }
    let opts = OpenOptions {
        password: v.password.clone(),
        attempt_recovery: v.allow_repairs,
        ..OpenOptions::default()
    };
    let doc = Document::open_path(path, &opts)
        .map_err(|e| fail(format!("qpdf cannot re-open the result: {e}")))?;
    if !v.allow_repairs && xref_was_repaired(&doc.repair_log()) {
        return Err(fail(format!(
            "qpdf had to repair the result: {}",
            doc.repair_log()
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    if let Some(n) = v.expect_pages {
        let got = doc
            .page_count()
            .map_err(|e| fail(format!("page tree unreadable: {e}")))?;
        if got != n {
            return Err(fail(format!("page count {got}, expected {n}")));
        }
    }
    drop(doc);
    if let Some(r) = v.renderer {
        r(path).map_err(|m| fail(format!("renderer: {m}")))?;
    }
    Ok(())
}

/// Replace `target` with the file `fill` produces at the temp path it is given.
///
/// `fill` receives a path that already exists as an empty file in the target's directory and must
/// leave the complete new content there. On any error the temp file is removed and `target` is
/// untouched.
pub fn atomic_replace(
    target: &Path,
    fill: impl FnOnce(&Path) -> Result<()>,
    mut opts: ReplaceOptions<'_>,
) -> Result<ReplaceOutcome> {
    // Replace the real file when the target is a symlink.
    let target = match fs::canonicalize(target) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => target.to_path_buf(),
        Err(e) => return Err(e.into()),
    };
    let mut fault = |step: Step| -> Result<()> {
        if let Some(f) = opts.faults.as_deref_mut() {
            f.before(step)?;
        }
        Ok(())
    };
    let tmp = temp_path(&target)?;

    let mut metadata_preserved = true;
    let staged = (|| -> Result<()> {
        fault(Step::CreateTemp)?;
        File::options().write(true).create_new(true).open(&tmp)?;
        fault(Step::Fill)?;
        fill(&tmp)?;
        fault(Step::SyncTemp)?;
        fsutil::full_sync(&File::options().read(true).write(true).open(&tmp)?)?;
        fault(Step::Validate)?;
        validate_file(&tmp, &opts.validation)?;
        fault(Step::CopyMetadata)?;
        if target.exists() {
            metadata_preserved = fsutil::copy_metadata(&target, &tmp)?;
        }
        fault(Step::Rename)?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fsutil::replace_file(&tmp, &target) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    // From here the target is the complete new file; a failed directory sync is reported, not
    // fatal.
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir_synced = fault(Step::SyncDir).is_ok() && fsutil::sync_dir(dir).is_ok();
    Ok(ReplaceOutcome {
        dir_synced,
        metadata_preserved,
    })
}
