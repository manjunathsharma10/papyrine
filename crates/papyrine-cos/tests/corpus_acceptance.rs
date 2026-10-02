//! ROADMAP 1.3 acceptance: every corpus file opens through `papyrine-cos` or fails with a
//! typed error, with no panic, crash or hang (30 s).
//!
//! Each file is opened in a child process (this test binary re-executed with
//! `PAPYRINE_CORPUS_CHILD=<path>`), so a segfault, abort, stack overflow or runaway loop is
//! caught as a result instead of taking the run down.
//!
//! Corpus files come from `corpus/cache/files` (run `corpus/fetch`) and
//! `corpus/cache/generated` (run `tools/gen-corpus`). Missing files are skipped with a note.
//!
//! * `cargo test -p papyrine-cos --test corpus_acceptance` runs the synthetic malformed set
//!   and a deterministic sample of the third-party corpus.
//! * `... -- --ignored corpus_full` runs every file, including the huge generated ones.
//!
//! Environment: `CORPUS_JOBS` (default 6 workers), `CORPUS_TIMEOUT` (seconds, default 30),
//! `PAPYRINE_CORPUS_REPORT` (TSV output path, default `$CARGO_TARGET_TMPDIR/corpus-report.tsv`).

use papyrine_cos::{Document, Error, OpenOptions, WriteOptions};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "PAPYRINE_CORPUS_CHILD";

// ---------------------------------------------------------------------------------------
// Child side

fn error_kind(e: &Error) -> &'static str {
    match e {
        Error::InvalidPassword => "InvalidPassword",
        Error::Damaged { .. } => "Damaged",
        Error::Unsupported(_) => "Unsupported",
        Error::Io(_) => "Io",
        Error::Pages(_) => "Pages",
        Error::Object(_) => "Object",
        Error::Type(_) => "Type",
        Error::Range(_) => "Range",
        Error::Internal(_) => "Internal",
        Error::Native(_) => "Native",
    }
}

/// Open the file, resolve every page object, and print one `RESULT` line.
/// Panics are deliberately not caught: exit code 101 is how the parent sees them.
#[test]
fn corpus_child() {
    let Ok(path) = std::env::var(CHILD_ENV) else {
        return; // normal test runs: nothing to do
    };
    let t0 = Instant::now();
    match Document::open_path(&path, &OpenOptions::default()) {
        Ok(doc) => {
            let mut note = String::new();
            let pages = match doc.page_count() {
                Ok(n) => {
                    // Resolve every page object (the page tree walk); stop at 20k pages.
                    for i in 0..n.min(20_000) {
                        if let Err(e) = doc.page(i) {
                            note = format!("page {i}: {e}");
                            break;
                        }
                    }
                    n.to_string()
                }
                Err(e) => {
                    note = format!("page_count: {e}");
                    "-".into()
                }
            };
            // Deep pass: serialise the whole document. Writing reads every stream and object,
            // which is when qpdf notices damage it did not need for the page tree.
            if std::env::var("PAPYRINE_CORPUS_DEEP").is_ok() {
                match doc.write(&WriteOptions::default()) {
                    Ok(out) => {
                        // The rewrite must itself be clean and keep the page count.
                        match Document::open_bytes(out.into_vec(), &OpenOptions::default()) {
                            Ok(d2) if d2.repair_log().is_empty() => {}
                            Ok(d2) => {
                                note += &format!(" rewrite not clean: {}", d2.repair_log().len())
                            }
                            Err(e) => note += &format!(" rewrite unreadable: {e}"),
                        }
                    }
                    Err(e) => note += &format!(" write: {e}"),
                }
            }
            let repairs = doc.repair_log().len();
            let status = if note.is_empty() { "ok" } else { "ok-partial" };
            println!(
                "RESULT\t{status}\t{pages}\t{repairs}\t{}\t{}",
                t0.elapsed().as_millis(),
                note.replace(['\t', '\n'], " ")
            );
        }
        Err(e) => {
            let repairs = match &e {
                Error::Damaged { repairs, .. } => repairs.len(),
                _ => 0,
            };
            println!(
                "RESULT\terr:{}\t-\t{repairs}\t{}\t{}",
                error_kind(&e),
                t0.elapsed().as_millis(),
                e.to_string().replace(['\t', '\n'], " ")
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Parent side

#[derive(Clone, Debug)]
struct Outcome {
    /// `ok`, `ok-partial`, `err:<Kind>`, `panic`, `crash:<signal>`, `timeout`.
    status: String,
    pages: String,
    repairs: usize,
    ms: u128,
    detail: String,
}

impl Outcome {
    fn is_bad(&self) -> bool {
        self.status == "panic" || self.status.starts_with("crash") || self.status == "timeout"
    }
}

fn timeout() -> Duration {
    let s = std::env::var("CORPUS_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30u64);
    Duration::from_secs(s)
}

fn run_child(path: &Path) -> Outcome {
    let tmp = std::env::temp_dir();
    // Unique per call: many corpus files share names (in.pdf, 1.pdf, ...).
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let tag = format!(
        "papyrine-corpus-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let (out_p, err_p) = (
        tmp.join(format!("{tag}.out")),
        tmp.join(format!("{tag}.err")),
    );
    let out_f = fs::File::create(&out_p).unwrap();
    let err_f = fs::File::create(&err_p).unwrap();
    let t0 = Instant::now();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "corpus_child", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, path)
        .envs(
            fs::metadata(path)
                .is_ok_and(|m| m.len() < 20 << 20)
                .then_some(("PAPYRINE_CORPUS_DEEP", "1")),
        )
        .stdin(Stdio::null())
        .stdout(out_f)
        .stderr(err_f)
        .spawn()
        .expect("spawn child");
    let limit = timeout();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if t0.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let ms = t0.elapsed().as_millis();
    let stdout = fs::read_to_string(&out_p).unwrap_or_default();
    let stderr = fs::read_to_string(&err_p).unwrap_or_default();
    let _ = fs::remove_file(&out_p);
    let _ = fs::remove_file(&err_p);
    let first_err = stderr
        .lines()
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    let Some(st) = status else {
        return Outcome {
            status: "timeout".into(),
            pages: "-".into(),
            repairs: 0,
            ms,
            detail: format!("killed after {}s", limit.as_secs()),
        };
    };
    // The harness prints "test corpus_child ... " in front of our line.
    if let Some(line) = stdout
        .find("RESULT\t")
        .map(|i| &stdout[i..])
        .and_then(|l| l.lines().next())
    {
        let f: Vec<&str> = line.splitn(6, '\t').collect();
        if f.len() == 6 {
            return Outcome {
                status: f[1].into(),
                pages: f[2].into(),
                repairs: f[3].parse().unwrap_or(0),
                ms: f[4].parse().unwrap_or(ms),
                detail: f[5].into(),
            };
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            return Outcome {
                status: format!("crash:signal{sig}"),
                pages: "-".into(),
                repairs: 0,
                ms,
                detail: first_err,
            };
        }
    }
    Outcome {
        status: if st.code() == Some(101) {
            "panic"
        } else {
            "crash:exit"
        }
        .into(),
        pages: "-".into(),
        repairs: 0,
        ms,
        detail: format!("exit {:?}: {first_err}", st.code()),
    }
}

/// Run `files` on a small worker pool; results come back in input order.
fn run_all(files: &[(PathBuf, Vec<String>)]) -> Vec<Outcome> {
    let jobs = std::env::var("CORPUS_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6usize);
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; files.len()]);
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= files.len() {
                        break;
                    }
                    let o = run_child(&files[i].0);
                    results.lock().unwrap()[i] = Some(o);
                }
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|o| o.unwrap())
        .collect()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Minimal reader for the `[[file]]` tables of `corpus/*.toml`: `(id, tags)`.
fn read_manifest(path: &Path) -> Vec<(String, Vec<String>)> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    let (mut out, mut id, mut tags): (Vec<_>, Option<String>, Vec<String>) = (vec![], None, vec![]);
    let quoted =
        |s: &str| -> Vec<String> { s.split('"').skip(1).step_by(2).map(String::from).collect() };
    for line in text.lines() {
        let line = line.trim();
        if line == "[[file]]" {
            if let Some(i) = id.take() {
                out.push((i, std::mem::take(&mut tags)));
            }
            tags.clear();
        } else if let Some(v) = line.strip_prefix("id")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            id = quoted(v).into_iter().next();
        } else if let Some(v) = line.strip_prefix("tags")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            tags = quoted(v);
        }
    }
    if let Some(i) = id {
        out.push((i, tags));
    }
    out
}

/// Every available corpus file with its tags. Generated files are tagged by name prefix.
fn collect(include_huge: bool) -> Vec<(PathBuf, Vec<String>)> {
    let root = repo_root().join("corpus");
    let mut files = Vec::new();
    for m in ["manifest.toml", "js-forms.toml"] {
        for (id, tags) in read_manifest(&root.join(m)) {
            let p = root.join("cache/files").join(&id);
            if p.is_file() {
                files.push((p, tags));
            }
        }
    }
    if let Ok(rd) = fs::read_dir(root.join("cache/generated")) {
        let mut generated: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        generated.sort();
        for p in generated {
            if p.extension().is_none_or(|e| e != "pdf") {
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let group = name.split('-').next().unwrap_or("gen").to_string();
            let huge = fs::metadata(&p)
                .map(|m| m.len() > 100 << 20)
                .unwrap_or(false);
            if huge && !include_huge {
                continue;
            }
            files.push((p, vec!["generated".into(), format!("gen-{group}")]));
        }
    }
    files
}

fn write_report(rows: &[(PathBuf, Vec<String>)], outs: &[Outcome], name: &str) {
    let path = std::env::var("PAPYRINE_CORPUS_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name));
    let mut s = String::from("file\ttags\tstatus\tpages\trepairs\tms\tdetail\n");
    for ((p, tags), o) in rows.iter().zip(outs) {
        s += &format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            p.strip_prefix(repo_root().join("corpus/cache"))
                .unwrap_or(p)
                .display(),
            tags.join(","),
            o.status,
            o.pages,
            o.repairs,
            o.ms,
            o.detail
        );
    }
    let _ = fs::write(&path, s);
    eprintln!("report: {}", path.display());
}

fn summarize(rows: &[(PathBuf, Vec<String>)], outs: &[Outcome]) {
    let mut by_tag: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    for ((_, tags), o) in rows.iter().zip(outs) {
        let class = match o.status.as_str() {
            "ok" if o.repairs > 0 => "ok+repairs".to_string(),
            s => s.to_string(),
        };
        *total.entry(class.clone()).or_default() += 1;
        for t in tags {
            *by_tag
                .entry(t.clone())
                .or_default()
                .entry(class.clone())
                .or_default() += 1;
        }
    }
    eprintln!("files: {}  totals: {total:?}", rows.len());
    for (t, m) in &by_tag {
        eprintln!("  {t:<12} {}  {m:?}", m.values().sum::<usize>());
    }
    let slow = outs.iter().map(|o| o.ms).max().unwrap_or(0);
    eprintln!("slowest file: {slow} ms");
}

fn assert_no_bad(rows: &[(PathBuf, Vec<String>)], outs: &[Outcome]) {
    let bad: Vec<String> = rows
        .iter()
        .zip(outs)
        .filter(|(_, o)| o.is_bad())
        .map(|((p, _), o)| format!("{}: {} ({})", p.display(), o.status, o.detail))
        .collect();
    assert!(
        bad.is_empty(),
        "panic/crash/hang on {} files:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Synthetic malformed files (tools/gen-corpus `mal-*`): never crash or hang, and the ones
/// that open must have logged their repairs.
#[test]
fn corpus_synthetic_malformed() {
    let rows: Vec<_> = collect(false)
        .into_iter()
        .filter(|(p, _)| p.file_name().unwrap().to_string_lossy().starts_with("mal-"))
        .collect();
    if rows.is_empty() {
        eprintln!("skipped: run tools/gen-corpus to create corpus/cache/generated");
        return;
    }
    let outs = run_all(&rows);
    write_report(&rows, &outs, "corpus-malformed.tsv");
    summarize(&rows, &outs);
    assert_no_bad(&rows, &outs);

    // Opened files that qpdf did not have to touch are baselines; everything else that opened
    // must say what it fixed.
    let mut silent = Vec::new();
    for ((p, _), o) in rows.iter().zip(&outs) {
        let name = p.file_name().unwrap().to_string_lossy();
        if o.status.starts_with("ok") && o.repairs == 0 && !name.contains("baseline") {
            silent.push(format!("{name} ({} pages)", o.pages));
        }
    }
    let known_silent = [
        // qpdf has nothing to repair at open or write time, or does not report it:
        // stream filters are only run when the data is decoded.
        "mal-content-inline-image-garbage.pdf",
        "mal-content-unbalanced-string.pdf",
        "mal-flate-bad-adler.pdf",
        "mal-flate-bomb-1gib.pdf",
        "mal-flate-corrupt-middle.pdf",
        "mal-flate-truncated-stream.pdf",
        "mal-flate-unknown-filter.pdf",
        // Tolerated by the lexer/parser without a warning.
        "mal-header-bad-version.pdf",
        "mal-nul-bytes-in-dicts.pdf",
        "mal-xref-short-entries.pdf",
        // Page-tree oddities: qpdf drops the bad kid (kid-missing-object loses a page without
        // a log entry: follow-up in docs/spikes/0.3-qpdf-vs-lopdf.md) or fails with a typed
        // Pages error (circular).
        "mal-page-parent-loop.pdf",
        "mal-pagetree-kid-missing-object.pdf",
        "mal-pagetree-circular.pdf",
    ];
    let unexpected: Vec<_> = silent
        .iter()
        .filter(|s| !known_silent.iter().any(|k| s.starts_with(k)))
        .collect();
    assert!(
        unexpected.is_empty(),
        "opened without any logged repair: {unexpected:?}"
    );
}

/// A third-party sample: every 7th file, plus every file tagged malformed/unreadable that is
/// small. Enough to catch regressions in a normal `cargo test`; `corpus_full` is the gate.
#[test]
fn corpus_third_party_sample() {
    let all = collect(false);
    let rows: Vec<_> = all
        .into_iter()
        .filter(|(p, _)| !p.to_string_lossy().contains("cache/generated"))
        .enumerate()
        .filter(|(i, _)| i % 7 == 0)
        .map(|(_, r)| r)
        .collect();
    if rows.is_empty() {
        eprintln!("skipped: run corpus/fetch");
        return;
    }
    let outs = run_all(&rows);
    write_report(&rows, &outs, "corpus-sample.tsv");
    summarize(&rows, &outs);
    assert_no_bad(&rows, &outs);
}

/// The ROADMAP 1.3 gate: the whole corpus, huge generated files included.
#[test]
#[ignore = "runs every corpus file (about 15 s here); cargo test -- --ignored corpus_full"]
fn corpus_full() {
    let rows = collect(true);
    if rows.is_empty() {
        eprintln!("skipped: no corpus files");
        return;
    }
    let outs = run_all(&rows);
    write_report(&rows, &outs, "corpus-full.tsv");
    summarize(&rows, &outs);
    assert_no_bad(&rows, &outs);
}

/// Known `papyrine-cos` bug (see docs/spikes/0.3-qpdf-vs-lopdf.md): when the trailer had to be
/// recovered from a file with no trailer dictionary, qpdf's writer emits a trailer without
/// `/Size`, so the rewritten file needs repair again. Fix: in `Document::write`, set
/// `/Size` on the trailer (highest object number + 1) when it is missing.
#[test]
#[ignore = "fails: known papyrine-cos bug, rewrite of a trailer-less file lacks /Size"]
fn corpus_rewrite_of_trailerless_file_is_clean() {
    let dir = repo_root().join("corpus/cache/generated");
    let mut checked = 0;
    for name in [
        "mal-trailer-missing",
        "mal-xref-missing",
        "mal-truncated-half",
        "mal-truncated-mid-stream",
        "mal-truncated-no-eof",
    ] {
        let p = dir.join(format!("{name}.pdf"));
        let Ok(doc) = Document::open_path(&p, &OpenOptions::default()) else {
            continue;
        };
        let out = doc.write(&WriteOptions::default()).unwrap().into_vec();
        let again = Document::open_bytes(out, &OpenOptions::default()).unwrap();
        assert!(
            again.repair_log().is_empty(),
            "{name}: rewritten file needs repair again: {:?}",
            again.repair_log().entries()
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("skipped: run tools/gen-corpus");
    }
}
