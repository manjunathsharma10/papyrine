//! Interop check for the three outputs (incremental section, ID-preserving full write, qpdf
//! optimized rewrite) on corpus files: `qpdf --check`, qpdf re-parse, and PDFium opening the file
//! and rendering page 1.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::*;
use papyrine_cos::{Document, ObjectStreams, OpenOptions, Secret, WriteOptions};
use papyrine_render as render;
use papyrine_writer::*;

fn pdfium_page1(
    lib: &Arc<render::Library>,
    bytes: Vec<u8>,
    pw: Option<&str>,
) -> Result<usize, String> {
    let mut d = render::Document::open(lib, render::bytes_from_vec(bytes), &[], pw)
        .map_err(|e| e.to_string())?;
    let n = d.page_count();
    if n > 0 {
        let t = d
            .render_preview(0, 160, &mut || false)
            .map_err(|e| e.to_string())?;
        if t.width == 0 || t.height == 0 {
            return Err("empty render".into());
        }
    }
    Ok(n)
}

/// Why `bytes` is not a good output; `None` when everything agrees with the original.
#[allow(clippy::unnecessary_to_owned)] // open_bytes needs an owned buffer
fn check_output(
    lib: &Arc<render::Library>,
    dir: &Path,
    label: &str,
    bytes: &[u8],
    pw: Option<&str>,
    pages: usize,
) -> Option<String> {
    let p = write_tmp(dir, &format!("{label}.pdf"), bytes);
    let (code, text) = qpdf_check(&p, pw);
    if code != 0 {
        return Some(format!(
            "{label}: qpdf --check exit {code}\n{}",
            text.lines().take(6).collect::<Vec<_>>().join("\n")
        ));
    }
    let opts = OpenOptions {
        password: pw.map(Secret::from),
        ..OpenOptions::default()
    };
    match Document::open_bytes(bytes.to_vec(), &opts) {
        Ok(d) if !d.repair_log().is_empty() => {
            return Some(format!("{label}: re-parse needed repairs"));
        }
        Ok(d) => {
            if d.page_count().ok() != Some(pages) {
                return Some(format!("{label}: page count changed"));
            }
        }
        Err(e) => return Some(format!("{label}: qpdf cannot reopen: {e}")),
    }
    match pdfium_page1(lib, bytes.to_vec(), pw) {
        Ok(n) if n == pages => None,
        Ok(n) => Some(format!("{label}: PDFium sees {n} pages, expected {pages}")),
        Err(e) => Some(format!("{label}: PDFium: {e}")),
    }
}

#[test]
fn three_outputs_interoperate_on_100_corpus_files() {
    let files = corpus_files(8 << 20);
    if files.is_empty() {
        eprintln!("corpus not fetched; skipping");
        return;
    }
    let Ok(lib) = render::Library::global() else {
        eprintln!("PDFium not available; skipping");
        return;
    };
    let want = env_usize("PAPYRINE_WRITER_INTEROP_N", 100);
    let dir = tempfile::tempdir().unwrap();
    let (mut done, mut scanned) = (0usize, 0usize);
    let mut failures = Vec::new();
    let mut seed = 0xC0FFEEu64;
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    // Every password-protected file first (they cover RC4/AES and encrypted object streams),
    // then an even sample of the rest.
    let (enc, rest): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| f.password.is_some());
    let candidates: Vec<CorpusFile> = enc.into_iter().chain(sample(&rest, want * 6)).collect();
    for f in candidates {
        if done >= want {
            break;
        }
        scanned += 1;
        let Ok(original) = std::fs::read(&f.path) else {
            continue;
        };
        let pw = f.password.as_deref();
        // Only files that are healthy to begin with: otherwise a failure is not ours.
        let Ok(chain) = ChainState::scan(original.as_slice()) else {
            continue;
        };
        let opts = OpenOptions {
            password: pw.map(Secret::from),
            attempt_recovery: false,
            ..OpenOptions::default()
        };
        let Ok(doc) = Document::open_bytes(original.clone(), &opts) else {
            continue;
        };
        let Ok(pages) = doc.page_count() else {
            continue;
        };
        if pages == 0 || !doc.repair_log().is_empty() {
            continue;
        }
        if qpdf_check(&f.path, pw).0 != 0 || pdfium_page1(&lib, original.clone(), pw) != Ok(pages) {
            continue;
        }
        let mut problems = Vec::new();

        // 1. Incremental section after random edits.
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut rng = Rng(seed | 1);
        let ids = doc.object_ids().unwrap();
        let dirty = random_edits(&doc, &mut rng, &ids, 3);
        let (inc, _) = incremental(&doc, &original, &dirty, &[]);
        problems.extend(check_output(
            &lib,
            dir.path(),
            "incremental",
            &inc,
            pw,
            pages,
        ));

        // 2. ID-preserving full write.
        let mut full = Vec::new();
        match write_full(
            &doc,
            &mut full,
            &FullWriteOptions {
                xref: chain.kind,
                ..Default::default()
            },
            &|| false,
        ) {
            Ok(_) => problems.extend(check_output(&lib, dir.path(), "full", &full, pw, pages)),
            Err(e) => problems.push(format!("full: {e}")),
        }

        // 3. qpdf optimized rewrite.
        let opt_path = dir.path().join("optimized-src.pdf");
        let wopts = WriteOptions {
            object_streams: ObjectStreams::Generate,
            ..WriteOptions::default()
        };
        match doc.write_to_path(&opt_path, &wopts) {
            Ok(_) => {
                let bytes = std::fs::read(&opt_path).unwrap();
                problems.extend(check_output(
                    &lib,
                    dir.path(),
                    "optimized",
                    &bytes,
                    pw,
                    pages,
                ));
            }
            Err(e) => problems.push(format!("optimized: {e}")),
        }

        *kinds
            .entry(if pw.is_some() { "encrypted" } else { "plain" })
            .or_default() += 1;
        *kinds
            .entry(if chain.kind == XrefKind::Stream {
                "xref-stream"
            } else {
                "xref-table"
            })
            .or_default() += 1;
        if !problems.is_empty() {
            failures.push(format!(
                "{}:\n  {}",
                f.path.display(),
                problems.join("\n  ")
            ));
        }
        done += 1;
    }
    eprintln!(
        "interop: {done} files checked ({scanned} scanned), mix {kinds:?}, {} with problems",
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(done >= want.min(60), "only {done} usable corpus files");
}
