//! ROADMAP 1.8 performance acceptance, measured through the real role:
//!
//! * first page < 500 ms on typical-20p and < 2 s on large-2000p-text
//!   (cold renderer process: spawn + handshake + open + first tile; and warm);
//! * edit -> updated tile < 100 ms p95 over 500 edits;
//! * renderer footprint growth < 10 MB over 500 edits.
//!
//! The assertions only run in release builds (`cargo test --release -p
//! papyrine-render --test perf -- --nocapture`); debug builds just print.
//! Corpus files come from `tools/gen-corpus`; without them the tests skip,
//! unless `PAPYRINE_REQUIRE_CORPUS` is set.

use papyrine_cos::{Document as Cos, OpenOptions};
use papyrine_ipc::*;
use papyrine_render::role::run_renderer;
use papyrine_render::testgen::{Built, Chain, stream_body};
use papyrine_render::{Library, mem};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type CC = ChildClient<RenderRequest, RenderResponse>;

#[test]
fn child_entry() {
    if std::env::var(spawn::ENV_ROLE).is_err() {
        return;
    }
    mem::reexec_with_child_env().expect("re-exec with allocator env");
    let _ = Library::global();
    let Some(b) = bootstrap().expect("bootstrap") else {
        return;
    };
    run_renderer(b.endpoint).expect("serve");
}

fn spawn_child() -> CC {
    let mut spec = ChildSpec::current_exe(Role::Renderer)
        .unwrap()
        .arg("child_entry")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1");
    spec.sandbox = true;
    CC::spawn(&spec, Duration::from_secs(60)).expect("spawn child")
}

fn corpus(name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/cache/generated")
        .join(name);
    if p.exists() {
        return Some(p);
    }
    assert!(
        std::env::var_os("PAPYRINE_REQUIRE_CORPUS").is_none(),
        "{name}: corpus/cache/generated is required (run tools/gen-corpus)"
    );
    eprintln!("perf: {name} not present, skipping");
    None
}

fn file_source(p: &Path) -> DocSource {
    DocSource::File {
        handle: Handle::from_file(std::fs::File::open(p).unwrap()),
        len: std::fs::metadata(p).unwrap().len(),
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn pool(cc: &CC) {
    let pool = SharedRegion::create(4 << 20).unwrap();
    cc.client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes: 1 << 20,
            slots: 4,
        })
        .unwrap();
}

fn first_tile(cc: &CC, doc: u64) {
    let r = cc
        .client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(doc),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(0),
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::Pixels(_)));
}

fn open(cc: &CC, doc: u64, path: &Path) {
    let r = cc
        .client
        .call_blocking(RenderRequest::Open {
            doc: DocId(doc),
            base: file_source(path),
            sections: vec![],
            password: None,
        })
        .unwrap();
    assert!(matches!(r, RenderResponse::Opened { .. }));
}

fn first_page(name: &str, budget_ms: f64) {
    let Some(path) = corpus(name) else { return };
    // Cold: a fresh renderer process, as at app start.
    let (mut cold, mut warm) = (vec![], vec![]);
    for _ in 0..5 {
        let t = Instant::now();
        let cc = spawn_child();
        pool(&cc);
        open(&cc, 1, &path);
        first_tile(&cc, 1);
        cold.push(ms(t));
        // Warm: the same process, another document.
        let t = Instant::now();
        open(&cc, 2, &path);
        first_tile(&cc, 2);
        warm.push(ms(t));
        let _ = cc.shutdown(Duration::from_secs(10));
    }
    let (c, w) = (pct(&mut cold.clone(), 0.5), pct(&mut warm.clone(), 0.5));
    eprintln!(
        "perf first page {name}: cold process median {c:.0} ms (runs {cold:.0?}), warm renderer median {w:.0} ms (budget {budget_ms:.0})"
    );
    if !cfg!(debug_assertions) {
        assert!(
            pct(&mut cold, 1.0) < budget_ms,
            "cold first page over budget"
        );
        assert!(pct(&mut warm, 1.0) < budget_ms);
    }
}

#[test]
fn first_page_typical_20p() {
    first_page("typical-20p.pdf", 500.0);
}

#[test]
fn first_page_large_2000p_text() {
    first_page("large-2000p-text.pdf", 2000.0);
}

/// What an edit chain needs from a corpus file.
struct EditInfo {
    startxref: usize,
    size: u32,
    root: u32,
    len: usize,
    content_ids: Vec<u32>,
}

fn edit_info(path: &Path, pages: usize) -> EditInfo {
    let mut f = std::fs::File::open(path).unwrap();
    let len = f.metadata().unwrap().len();
    let n = len.min(4096);
    f.seek(SeekFrom::Start(len - n)).unwrap();
    let mut tail = vec![0u8; n as usize];
    f.read_exact(&mut tail).unwrap();
    let t = String::from_utf8_lossy(&tail).into_owned();
    let i = t.rfind("startxref").unwrap() + "startxref".len();
    let startxref = t[i..]
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let doc = Cos::open_path(path, &OpenOptions::default()).unwrap();
    let tr = doc.trailer().unwrap();
    let size = tr.dict_get("Size").unwrap().as_int().unwrap() as u32;
    let root = tr.dict_get("Root").unwrap().id().unwrap().num;
    let content_ids = (0..pages.min(doc.page_count().unwrap()))
        .map(|i| {
            let c = doc.page(i).unwrap().dict_get("Contents").unwrap();
            c.id().or_else(|| c.array_get(0).unwrap().id()).unwrap().num
        })
        .collect();
    EditInfo {
        startxref,
        size,
        root,
        len: len as usize,
        content_ids,
    }
}

/// 500 edits, each a new content stream for one page, section chain compacted (L1)
/// at k > 16. Returns per-edit milliseconds from `Reopen` sent to the tile received,
/// and the renderer's footprint before and after (current, lifetime peak).
fn edit_session(name: &str, edits: usize) -> Option<(Vec<f64>, u64, u64, u64)> {
    let path = corpus(name)?;
    let info = edit_info(&path, 64);
    let base = Built {
        bytes: Vec::new(),
        startxref: info.startxref,
        size: info.size,
        root: info.root,
        page_ids: vec![],
        content_ids: info.content_ids.clone(),
    };
    let fresh = || {
        let mut c = Chain::new(&base);
        c.total_len = info.len;
        c
    };
    let cc = spawn_child();
    pool(&cc);
    open(&cc, 1, &path);
    let pid = cc.process.id();
    for p in 0..8 {
        let _ = cc.client.call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: p,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(0),
        });
    }
    let before = mem::footprint_of(pid).map_or(0, |m| m.0);

    let mut chain = fresh();
    let mut secs: Vec<Vec<u8>> = Vec::new();
    let mut dirty: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut times = Vec::with_capacity(edits);
    for i in 0..edits {
        let page = (i * 7) % info.content_ids.len();
        let cid = info.content_ids[page];
        let content = format!(
            "{:.2} 0.3 0.7 rg 30 30 {} 200 re f\nBT /F1 14 Tf 40 700 Td (edit {i}) Tj ET\n",
            (i % 9) as f32 / 9.0,
            100 + i % 200
        );
        let body = stream_body("", content.as_bytes());
        dirty.insert(cid, body.clone());
        if secs.len() >= 16 {
            chain = fresh();
            let all: Vec<(u32, Vec<u8>)> = dirty.iter().map(|(k, v)| (*k, v.clone())).collect();
            secs = vec![chain.section(&all)];
        } else {
            secs.push(chain.section(&[(cid, body)]));
        }
        let srcs: Vec<DocSource> = secs
            .iter()
            .map(|s| {
                let r = SharedRegion::create(s.len()).unwrap();
                r.write_at(0, s);
                DocSource::Shared {
                    region: r,
                    len: s.len() as u64,
                }
            })
            .collect();
        let t = Instant::now();
        let re = cc
            .client
            .call(RenderRequest::Reopen {
                doc: DocId(1),
                sections: srcs,
            })
            .unwrap();
        let r = cc.client.call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: page as u32,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot((i % 4) as u32),
        });
        assert!(matches!(r, Ok(RenderResponse::Pixels(_))), "{r:?}");
        times.push(ms(t));
        re.wait().unwrap();
    }
    let (after, peak) = mem::footprint_of(pid).unwrap_or((0, 0));
    let _ = cc.shutdown(Duration::from_secs(10));
    Some((times, before, after, peak))
}

fn edits_report(name: &str) {
    let Some((mut times, before, after, peak)) = edit_session(name, 500) else {
        return;
    };
    let (p50, p95, max) = (
        pct(&mut times, 0.5),
        pct(&mut times, 0.95),
        pct(&mut times, 1.0),
    );
    // The lifetime peak bounds the footprint at every moment of the session, so
    // `peak - before` is the strictest reading of "growth over 500 edits".
    let growth = peak.saturating_sub(before) as f64 / (1024.0 * 1024.0);
    eprintln!(
        "perf 500 edits {name}: edit->tile p50 {p50:.1} ms p95 {p95:.1} ms max {max:.1} ms; renderer footprint {:.1} MB -> {:.1} MB (peak growth {growth:.1} MB, lifetime peak {:.1} MB)",
        before as f64 / 1048576.0,
        after as f64 / 1048576.0,
        peak as f64 / 1048576.0
    );
    if !cfg!(debug_assertions) {
        assert!(p95 < 100.0, "edit -> tile p95 {p95:.1} ms");
        assert!(
            growth < 10.0,
            "renderer growth {growth:.1} MB over 500 edits"
        );
    }
}

#[test]
fn edits_typical_20p() {
    edits_report("typical-20p.pdf");
}

#[test]
fn edits_large_2000p_text() {
    edits_report("large-2000p-text.pdf");
}
