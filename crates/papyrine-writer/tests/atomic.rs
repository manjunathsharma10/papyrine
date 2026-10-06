//! Fault injection at every atomic-replace step: the target is always either the complete old
//! file or the complete new file, never anything in between.

mod common;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use papyrine_cos::ObjectStreams;
use papyrine_writer::*;

struct FailAt(Step);

impl FaultInjector for FailAt {
    fn before(&mut self, step: Step) -> io::Result<()> {
        if step == self.0 {
            Err(io::Error::other(format!(
                "injected failure before {step:?}"
            )))
        } else {
            Ok(())
        }
    }
}

struct AbortAt(Step);

impl FaultInjector for AbortAt {
    fn before(&mut self, step: Step) -> io::Result<()> {
        if step == self.0 {
            std::process::abort();
        }
        Ok(())
    }
}

// qpdf derives the trailer /ID from the clock, so two builds of the "same" file can differ
// when they straddle a second boundary; build each fixture once per process.
fn pdf_a() -> Vec<u8> {
    static A: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    A.get_or_init(|| make_pdf(2, ObjectStreams::Disable, None))
        .clone()
}

fn pdf_b() -> Vec<u8> {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| make_pdf(5, ObjectStreams::Generate, None))
        .clone()
}

fn replace_with(
    target: &Path,
    new: &[u8],
    faults: Option<&mut dyn FaultInjector>,
) -> Result<ReplaceOutcome> {
    atomic_replace(
        target,
        |tmp| {
            std::fs::write(tmp, new)?;
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                expect_pages: Some(5),
                ..Validation::default()
            },
            faults,
        },
    )
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn assert_valid_pdf(path: &Path, pages: usize) {
    let doc =
        papyrine_cos::Document::open_path(path, &papyrine_cos::OpenOptions::default()).unwrap();
    assert!(doc.repair_log().is_empty());
    assert_eq!(doc.page_count().unwrap(), pages);
}

#[test]
fn success_replaces_and_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("doc.pdf");
    std::fs::write(&target, pdf_a()).unwrap();
    let out = replace_with(&target, &pdf_b(), None).unwrap();
    assert!(out.dir_synced);
    assert_eq!(std::fs::read(&target).unwrap(), pdf_b());
    assert_eq!(dir_listing(dir.path()), ["doc.pdf"]);
}

#[test]
fn error_at_every_step_keeps_the_target_whole() {
    for step in Step::ALL {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("doc.pdf");
        std::fs::write(&target, pdf_a()).unwrap();
        let mut f = FailAt(step);
        let r = replace_with(&target, &pdf_b(), Some(&mut f));
        let content = std::fs::read(&target).unwrap();
        if step == Step::SyncDir {
            // The rename already happened: the new file is in place, durability unconfirmed.
            let o = r.unwrap();
            assert!(!o.dir_synced && o.metadata_preserved);
            assert_eq!(content, pdf_b(), "{step:?}");
        } else {
            assert!(r.is_err(), "{step:?}");
            assert_eq!(content, pdf_a(), "{step:?}: target changed");
        }
        assert_eq!(
            dir_listing(dir.path()),
            ["doc.pdf"],
            "{step:?}: temp file left behind"
        );
        assert_valid_pdf(&target, if step == Step::SyncDir { 5 } else { 2 });
    }
}

#[test]
fn failing_producer_and_failing_validation_keep_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("doc.pdf");
    std::fs::write(&target, pdf_a()).unwrap();
    let b = pdf_b();

    // Producer dies half way.
    let r = atomic_replace(
        &target,
        |tmp| {
            std::fs::write(tmp, &b[..b.len() / 2])?;
            Err(Error::Io(io::Error::other("disk full")))
        },
        ReplaceOptions::default(),
    );
    assert!(r.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), pdf_a());

    // Producer "succeeds" with a truncated file: validation refuses it.
    let r = atomic_replace(
        &target,
        |tmp| {
            std::fs::write(tmp, &b[..b.len() / 2])?;
            Ok(())
        },
        ReplaceOptions::default(),
    );
    assert!(matches!(r, Err(Error::Validation(_))), "{r:?}");
    // Right file, wrong page count.
    let r = atomic_replace(
        &target,
        |tmp| {
            std::fs::write(tmp, &b)?;
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                expect_pages: Some(99),
                ..Validation::default()
            },
            faults: None,
        },
    );
    assert!(matches!(r, Err(Error::Validation(_))));
    // The host's renderer check can veto too.
    let veto: &dyn Fn(&Path) -> Result<(), String> = &|_| Err("page 1 failed to render".into());
    let r = atomic_replace(
        &target,
        |tmp| {
            std::fs::write(tmp, &b)?;
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                renderer: Some(veto),
                ..Validation::default()
            },
            faults: None,
        },
    );
    assert!(
        matches!(&r, Err(Error::Validation(m)) if m.contains("render")),
        "{r:?}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), pdf_a());
    assert_eq!(dir_listing(dir.path()), ["doc.pdf"]);
}

/// Re-entered as a child process by `process_kill_at_every_step`; does nothing on its own.
#[test]
#[ignore = "helper run as a child process"]
fn abort_child() {
    let Ok(dir) = std::env::var("PAPYRINE_FAULT_DIR") else {
        return;
    };
    let step = std::env::var("PAPYRINE_FAULT_STEP").unwrap();
    let target = PathBuf::from(dir).join("doc.pdf");
    let b = pdf_b();
    if step == "MidFill" {
        let _ = atomic_replace(
            &target,
            |tmp| {
                std::fs::write(tmp, &b[..b.len() / 2])?;
                std::process::abort();
            },
            ReplaceOptions::default(),
        );
        return;
    }
    let step = Step::ALL
        .into_iter()
        .find(|s| format!("{s:?}") == step)
        .unwrap();
    let mut f = AbortAt(step);
    let _ = replace_with(&target, &b, Some(&mut f));
}

#[test]
fn process_kill_at_every_step() {
    let exe = std::env::current_exe().unwrap();
    let steps: Vec<String> = Step::ALL
        .iter()
        .map(|s| format!("{s:?}"))
        .chain(["MidFill".to_string()])
        .collect();
    for step in steps {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("doc.pdf");
        std::fs::write(&target, pdf_a()).unwrap();
        let status = Command::new(&exe)
            .args([
                "--exact",
                "abort_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PAPYRINE_FAULT_DIR", dir.path())
            .env("PAPYRINE_FAULT_STEP", &step)
            .output()
            .unwrap()
            .status;
        assert!(!status.success(), "{step}: child should have been killed");
        let content = std::fs::read(&target).unwrap();
        if step == "SyncDir" {
            assert_eq!(content, pdf_b(), "{step}");
            assert_valid_pdf(&target, 5);
        } else {
            assert_eq!(content, pdf_a(), "{step}: target changed by a crash");
            assert_valid_pdf(&target, 2);
        }
        // A crash can leave a temp file; it is recognisable and removable.
        let removed = cleanup_stale_temps(&target).unwrap();
        assert!(removed <= 1, "{step}");
        assert_eq!(dir_listing(dir.path()), ["doc.pdf"], "{step}");
    }
}

#[test]
fn creates_the_target_for_save_as_and_follows_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("new.pdf");
    replace_with(&new, &pdf_b(), None).unwrap();
    assert_eq!(std::fs::read(&new).unwrap(), pdf_b());

    #[cfg(unix)]
    {
        let real = dir.path().join("real.pdf");
        let link = dir.path().join("link.pdf");
        std::fs::write(&real, pdf_a()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        replace_with(&link, &pdf_b(), None).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&real).unwrap(), pdf_b());
    }
}

#[cfg(unix)]
#[test]
fn permissions_and_extended_attributes_survive() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("doc.pdf");
    std::fs::write(&target, pdf_a()).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    let have_xattr = set_xattr(&target, "user.papyrine", "tagged");
    replace_with(&target, &pdf_b(), None).unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    if have_xattr {
        assert_eq!(
            get_xattr(&target, "user.papyrine").as_deref(),
            Some("tagged")
        );
    } else {
        eprintln!("no xattr tooling; extended attribute check skipped");
    }
}

#[cfg(unix)]
fn set_xattr(path: &Path, name: &str, value: &str) -> bool {
    let (cmd, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        ("xattr", vec!["-w".into(), name.into(), value.into()])
    } else {
        (
            "setfattr",
            vec!["-n".into(), name.into(), "-v".into(), value.into()],
        )
    };
    Command::new(cmd)
        .args(args)
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
fn get_xattr(path: &Path, name: &str) -> Option<String> {
    let out = if cfg!(target_os = "macos") {
        Command::new("xattr")
            .args(["-p", name])
            .arg(path)
            .output()
            .ok()?
    } else {
        Command::new("getfattr")
            .args(["--only-values", "-n", name])
            .arg(path)
            .output()
            .ok()?
    };
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
