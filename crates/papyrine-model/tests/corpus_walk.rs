//! Walk every cached corpus file through every view: no panics, no hangs (30 s per file).
//! Skipped when `corpus/cache` is absent. Run with `--release -- --nocapture` for the stats.
//!
//! `PAPYRINE_CORPUS_FILTER=substr` restricts the walk; `PAPYRINE_CORPUS_JOBS=n` sets threads.

use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use papyrine_cos::{Error, OpenOptions};
use papyrine_model::Model;

const LIMIT: Duration = Duration::from_secs(30);

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/cache")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e == "pdf") {
            out.push(p);
        }
    }
}

fn passwords() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(t) = std::fs::read_to_string(corpus_root().join("generated/passwords.tsv")) {
        for l in t.lines().skip(1) {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() >= 3 {
                // The owner password always opens the file and exercises the owner path.
                m.insert(c[0].to_string(), c[2].to_string());
            }
        }
    }
    m
}

#[derive(Default, Debug)]
struct Stats {
    files: usize,
    opened: usize,
    open_failed: BTreeMap<String, usize>,
    view_errors: BTreeMap<String, usize>,
    panics: Vec<String>,
    pages: usize,
    fields: usize,
    widgets: usize,
    annots: usize,
    outline_items: usize,
    signed: usize,
    with_js: usize,
    encrypted: usize,
    xfa_static: usize,
    xfa_dynamic: usize,
    truncated_walks: usize,
    slowest: Vec<(Duration, String)>,
}

/// Exercise every view of one opened model. Returns counts through `st`.
fn walk(m: &Model, st: &mut Stats) {
    macro_rules! view {
        ($name:expr, $e:expr) => {
            match $e {
                Ok(v) => Some(v),
                Err(e) => {
                    *st.view_errors
                        .entry(format!("{}: {}", $name, kind(&e)))
                        .or_default() += 1;
                    None
                }
            }
        };
    }
    let n = view!("page_count", m.page_count()).unwrap_or(0);
    st.pages += n;
    // All pages of ordinary files; a spread sample of huge ones.
    let idx: Vec<usize> = if n <= 300 {
        (0..n).collect()
    } else {
        (0..40).chain((0..n).step_by(37)).chain(n - 40..n).collect()
    };
    for &i in &idx {
        let p = view!("page", m.page(i));
        if let Some(p) = p {
            let s = p.display_size();
            assert!(s.w.is_finite() && s.h.is_finite());
        }
        if let Some(a) = view!("annotations", m.annotations(i)) {
            st.annots += a.items.len();
        }
    }
    if let Some(l) = view!("labels", m.page_labels()) {
        for &i in idx.iter().take(50) {
            let _ = l.label(i);
        }
    }
    if let Some(o) = view!("outlines", m.outlines()) {
        st.outline_items += o.total();
        st.truncated_walks += usize::from(o.truncated);
    }
    if let Some(f) = view!("form", m.form()) {
        st.fields += f.terminal_fields().count();
        st.widgets += f.widget_count();
        match f.xfa {
            papyrine_model::XfaKind::Static => st.xfa_static += 1,
            papyrine_model::XfaKind::Dynamic => st.xfa_dynamic += 1,
            _ => {}
        }
    }
    if let Some(s) = view!("signatures", m.signatures()) {
        st.signed += usize::from(s.is_signed());
    }
    if let Some(s) = view!("scripts", m.scripts()) {
        st.with_js += usize::from(s.has_javascript());
    }
    let _ = view!("info", m.info());
    let _ = view!("catalog", m.catalog_info());
    let _ = view!("named_dests", m.named_destinations());
    let _ = view!("xmp", m.xmp_packet());
    if let Some(s) = view!("security", m.security()) {
        st.encrypted += usize::from(s.encrypted);
    }
}

fn kind(e: &Error) -> &'static str {
    match e {
        Error::InvalidPassword => "password",
        Error::Damaged { .. } => "damaged",
        Error::Unsupported(_) => "unsupported",
        Error::Io(_) => "io",
        Error::Pages(_) => "pages",
        Error::Object(_) => "object",
        Error::Type(_) => "type",
        Error::Range(_) => "range",
        Error::Internal(_) => "internal",
        Error::Native(_) => "native",
    }
}

#[test]
fn every_corpus_file_walks_without_panics() {
    let root = corpus_root();
    if !root.exists() {
        eprintln!("corpus/cache absent: skipping");
        return;
    }
    let mut files = Vec::new();
    collect(&root, &mut files);
    if let Ok(f) = std::env::var("PAPYRINE_CORPUS_FILTER") {
        files.retain(|p| p.to_string_lossy().contains(&f));
    }
    if files.is_empty() {
        eprintln!("no corpus files: skipping");
        return;
    }
    let pw = Arc::new(passwords());
    let files = Arc::new(files);
    let next = Arc::new(AtomicUsize::new(0));
    let stats = Arc::new(Mutex::new(Stats::default()));
    // (start, name) of the file each worker is on, for the watchdog.
    let running: Arc<Mutex<HashMap<usize, (Instant, String)>>> = Arc::default();
    let done = Arc::new(AtomicUsize::new(0));
    let jobs = std::env::var("PAPYRINE_CORPUS_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or(4usize);

    let mut handles = Vec::new();
    for w in 0..jobs {
        let (files, next, stats, pw, running, done) = (
            files.clone(),
            next.clone(),
            stats.clone(),
            pw.clone(),
            running.clone(),
            done.clone(),
        );
        handles.push(std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(path) = files.get(i) else { break };
                let name = path
                    .strip_prefix(corpus_root())
                    .unwrap_or(path)
                    .display()
                    .to_string();
                running
                    .lock()
                    .unwrap()
                    .insert(w, (Instant::now(), name.clone()));
                let t0 = Instant::now();
                let mut local = Stats {
                    files: 1,
                    ..Stats::default()
                };
                let base = path.file_name().unwrap().to_string_lossy().into_owned();
                let opts = match pw.get(&base) {
                    Some(p) => OpenOptions::with_password(p.as_str()),
                    None => OpenOptions::default(),
                };
                let r = catch_unwind(AssertUnwindSafe(|| match Model::open_path(path, &opts) {
                    Ok(m) => {
                        walk(&m, &mut local);
                        true
                    }
                    Err(e) => {
                        *local.open_failed.entry(kind(&e).to_string()).or_default() += 1;
                        false
                    }
                }));
                match r {
                    Ok(true) => local.opened = 1,
                    Ok(false) => {}
                    Err(p) => {
                        let msg = p
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        local.panics.push(format!("{name}: {msg}"));
                    }
                }
                let el = t0.elapsed();
                local.slowest.push((el, name));
                running.lock().unwrap().remove(&w);
                let mut s = stats.lock().unwrap();
                merge(&mut s, local);
                done.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    // Watchdog: a hang cannot be unwound, so report and abort.
    let total = files.len();
    while done.load(Ordering::SeqCst) < total {
        std::thread::sleep(Duration::from_millis(200));
        for (start, name) in running.lock().unwrap().values() {
            if start.elapsed() > LIMIT + Duration::from_secs(5) {
                eprintln!("TIMEOUT (> {LIMIT:?}): {name}");
                std::process::abort();
            }
        }
    }
    for h in handles {
        h.join().unwrap();
    }
    let mut s = stats.lock().unwrap();
    s.slowest.sort_by_key(|a| std::cmp::Reverse(a.0));
    s.slowest.truncate(8);
    eprintln!("corpus walk: {s:#?}");
    assert!(s.panics.is_empty(), "panics: {:#?}", s.panics);
    let over: Vec<_> = s.slowest.iter().filter(|(d, _)| *d > LIMIT).collect();
    assert!(over.is_empty(), "files over the time limit: {over:?}");
}

fn merge(a: &mut Stats, b: Stats) {
    a.files += b.files;
    a.opened += b.opened;
    for (k, v) in b.open_failed {
        *a.open_failed.entry(k).or_default() += v;
    }
    for (k, v) in b.view_errors {
        *a.view_errors.entry(k).or_default() += v;
    }
    a.panics.extend(b.panics);
    a.pages += b.pages;
    a.fields += b.fields;
    a.widgets += b.widgets;
    a.annots += b.annots;
    a.outline_items += b.outline_items;
    a.signed += b.signed;
    a.with_js += b.with_js;
    a.encrypted += b.encrypted;
    a.xfa_static += b.xfa_static;
    a.xfa_dynamic += b.xfa_dynamic;
    a.truncated_walks += b.truncated_walks;
    a.slowest.extend(b.slowest);
    a.slowest.sort_by_key(|x| std::cmp::Reverse(x.0));
    a.slowest.truncate(64);
}
