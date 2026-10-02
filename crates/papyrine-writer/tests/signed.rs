//! Incremental saves of signed files: the original bytes stay an exact prefix and pyHanko
//! (a test-only oracle installed in a venv under target/) still reports the signature intact.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use common::*;
use papyrine_cos::{Document, ObjectStreams, OpenOptions, WriteOptions};
use papyrine_writer::*;

struct Oracle {
    python: PathBuf,
    script: PathBuf,
    work: PathBuf,
}

fn target_dir() -> PathBuf {
    // <target>/<profile>/deps/<exe>
    let exe = std::env::current_exe().unwrap();
    exe.ancestors().nth(3).unwrap().to_path_buf()
}

fn oracle() -> Option<&'static Oracle> {
    static O: OnceLock<Option<Oracle>> = OnceLock::new();
    O.get_or_init(|| {
        let venv = target_dir().join("pyhanko-venv");
        let python = venv.join("bin/python");
        if !python.exists() {
            let ok = Command::new("python3").args(["-m", "venv"]).arg(&venv).status().is_ok_and(|s| s.success())
                && Command::new(venv.join("bin/pip"))
                    .args(["install", "-q", "pyhanko", "pyhanko-certvalidator"])
                    .status()
                    .is_ok_and(|s| s.success());
            if !ok {
                let _ = std::fs::remove_dir_all(&venv);
                return None;
            }
        }
        let work = target_dir().join("pyhanko-work");
        std::fs::create_dir_all(&work).ok()?;
        Some(Oracle {
            python,
            script: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/pyhanko_oracle.py"),
            work,
        })
    })
    .as_ref()
}

impl Oracle {
    fn sign(&self, input: &Path, output: &Path) {
        let s = Command::new(&self.python)
            .arg(&self.script)
            .args(["sign"])
            .arg(input)
            .arg(output)
            .arg(&self.work)
            .output()
            .unwrap();
        assert!(s.status.success(), "pyHanko sign failed: {}", String::from_utf8_lossy(&s.stderr));
    }

    /// `(field, intact, valid, coverage, modification level)` per signature, or an error text.
    fn verify(&self, pdf: &Path) -> Vec<String> {
        let o = Command::new(&self.python).arg(&self.script).arg("verify").arg(pdf).arg(&self.work).output().unwrap();
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        text.lines().map(String::from).collect()
    }
}

fn json_bool(s: &str, key: &str) -> bool {
    s.contains(&format!("\"{key}\": true"))
}

#[test]
fn signed_files_keep_their_signature_after_incremental_save() {
    let Some(o) = oracle() else {
        eprintln!("pyHanko venv unavailable (offline?); skipping");
        return;
    };
    for (name, streams) in [("table", ObjectStreams::Disable), ("xref-stream", ObjectStreams::Generate)] {
        let dir = tempfile::tempdir().unwrap();
        let plain = write_tmp(dir.path(), "plain.pdf", &make_pdf(2, streams, None));
        let signed = dir.path().join("signed.pdf");
        o.sign(&plain, &signed);
        let before = o.verify(&signed);
        assert!(json_bool(&before[0], "intact") && json_bool(&before[0], "valid"), "{name}: {before:?}");

        let doc = Document::open_path(&signed, &OpenOptions::default()).unwrap();
        assert!(has_signatures(&doc).unwrap(), "{name}");
        assert!(matches!(plan(&doc, &std::fs::File::open(&signed).unwrap()).unwrap(), SavePlan::Incremental { .. }));

        // Edit: a text annotation on page 1 and a new Info entry.
        let page = doc.page(0).unwrap();
        let annot = doc
            .parse_object("<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (Reviewed) /Name /Comment /F 4 >>")
            .unwrap();
        let annot = doc.make_indirect(&annot).unwrap();
        let annots = doc.new_array();
        annots.array_push(&annot).unwrap();
        page.dict_set("Annots", &annots).unwrap();
        let info = doc.trailer().unwrap().dict_get("Info").unwrap();
        info.dict_set("Subject", &doc.new_string("edited after signing").unwrap()).unwrap();
        let dirty = vec![page.id().unwrap(), annot.id().unwrap(), info.id().unwrap()];

        let out = dir.path().join("saved.pdf");
        let report = save_incremental(
            &doc,
            &signed,
            &out,
            &SectionRequest { dirty, ..SectionRequest::default() },
            SaveOptions::default(),
        )
        .unwrap();
        assert_eq!(report.kind, SaveKind::Incremental);

        let original = std::fs::read(&signed).unwrap();
        let saved = std::fs::read(&out).unwrap();
        assert!(saved.len() > original.len());
        assert_eq!(&saved[..original.len()], &original[..], "{name}: original bytes must be an exact prefix");

        let after = o.verify(&out);
        eprintln!("{name}: pyHanko after save: {after:?}");
        assert!(json_bool(&after[0], "intact"), "{name}: signature no longer intact: {after:?}");
        assert!(json_bool(&after[0], "valid"), "{name}: signature no longer valid: {after:?}");
        assert!(after[0].contains("ENTIRE_REVISION"), "{name}: {after:?}");

        // The edit is really there.
        let re = Document::open_path(&out, &OpenOptions::default()).unwrap();
        assert_eq!(re.page(0).unwrap().dict_get("Annots").unwrap().array_len().unwrap(), 1);

        // Control: the optimized rewrite renumbers and recompresses, which does break the
        // signature. This is why signed files never take that path without asking.
        let rewritten = dir.path().join("rewritten.pdf");
        save_optimized(&doc, &rewritten, &WriteOptions::default(), SaveOptions::default()).unwrap();
        let broken = o.verify(&rewritten);
        assert!(
            broken.iter().all(|l| !(json_bool(l, "intact") && json_bool(l, "valid"))),
            "{name}: control rewrite unexpectedly kept the signature: {broken:?}"
        );
    }
}
