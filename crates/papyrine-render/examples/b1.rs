//! Benchmark B1-renderer (ARCHITECTURE §4.6): re-open time for base + k sections
//! of size S, time-to-first-tile, edit-session latency and memory.
//!
//!   cargo run --release -p papyrine-render --example b1 -- [--quick] [--dir DIR]
//!
//! Generated PDFs go to `third_party/cache/render-bench/` (gitignored). Output is
//! markdown on stdout. Memory is `phys_footprint` on macOS (the number Activity
//! Monitor and the budget gate use), VmRSS elsewhere on Linux, n/a otherwise.

use papyrine_render::testgen::*;
use papyrine_render::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod mem {
    #[repr(C)]
    struct TaskVmInfo {
        virtual_size: u64,
        region_count: i32,
        page_size: i32,
        resident_size: u64,
        resident_size_peak: u64,
        device: u64,
        device_peak: u64,
        internal: u64,
        internal_peak: u64,
        external: u64,
        external_peak: u64,
        reusable: u64,
        reusable_peak: u64,
        purgeable_volatile_pmap: u64,
        purgeable_volatile_resident: u64,
        purgeable_volatile_virtual: u64,
        compressed: u64,
        compressed_peak: u64,
        compressed_lifetime: u64,
        phys_footprint: u64,
        _rest: [u64; 32],
    }
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut TaskVmInfo, count: *mut u32) -> i32;
    }
    const TASK_VM_INFO: u32 = 22;

    /// (phys_footprint, peak resident) in bytes.
    pub fn sample() -> Option<(u64, u64)> {
        // SAFETY: plain Mach call into a zeroed, correctly sized out struct.
        unsafe {
            let mut info: TaskVmInfo = std::mem::zeroed();
            let mut count = (std::mem::size_of::<TaskVmInfo>() / 4) as u32;
            if task_info(mach_task_self(), TASK_VM_INFO, &mut info, &mut count) != 0 {
                return None;
            }
            Some((info.phys_footprint, info.resident_size_peak))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod mem {
    pub fn sample() -> Option<(u64, u64)> {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = |k: &str| -> Option<u64> {
            s.lines()
                .find(|l| l.starts_with(k))?
                .split_whitespace()
                .nth(1)?
                .parse::<u64>()
                .ok()
                .map(|v| v * 1024)
        };
        Some((kb("VmRSS:")?, kb("VmHWM:")?))
    }
}

fn footprint() -> u64 {
    mem::sample().map_or(0, |m| m.0)
}

fn mb(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1]
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Parameters of a generated document.
const DOCS: [(&str, usize, usize, usize); 2] = [
    ("typical-20p", 20, 50, 300),
    ("large-2000p-text", 2000, 50, 600),
];

/// Write the base file (run in a child process so the harness heap stays clean).
fn write_doc(path: &Path, pages: usize, lines: usize, rects: usize) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let specs: Vec<PageSpec> = (0..pages)
        .map(|i| PageSpec::letter(busy_content(i, lines, rects, &mut rng)))
        .collect();
    fs::write(path, build(&specs).bytes).unwrap();
}

/// Object ids and chain start for a file written by `write_doc` (ids follow
/// `testgen::build`'s numbering; `startxref` is read from the file tail).
fn doc_info(path: &Path, pages: usize) -> (Built, Chain) {
    let bytes = open_mmap(path).unwrap();
    let all: &[u8] = (*bytes).as_ref();
    let tail = String::from_utf8_lossy(&all[all.len().saturating_sub(64)..]).into_owned();
    let startxref: usize = tail
        .rsplit("startxref")
        .next()
        .and_then(|t| t.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("startxref");
    let built = Built {
        bytes: Vec::new(),
        startxref,
        size: 4 + 2 * pages as u32,
        root: 1,
        page_ids: (0..pages as u32).map(|i| 4 + 2 * i).collect(),
        content_ids: (0..pages as u32).map(|i| 5 + 2 * i).collect(),
    };
    let chain = Chain {
        total_len: all.len(),
        startxref,
        size: built.size,
        root: 1,
    };
    (built, chain)
}

fn color_content(i: usize) -> Vec<u8> {
    let c = (i % 7) as f32 / 7.0;
    format!(
        "{c:.2} {:.2} 0.5 rg 100 100 200 200 re f\nBT /F1 24 Tf 72 700 Td (edit {i}) Tj ET\n",
        1.0 - c
    )
    .into_bytes()
}

struct Cell {
    open_ms: f64,
    reopen_ms: f64,
    tile_ms: f64,
    tile_mid_ms: f64,
    rss_open: f64,
    rss_tile: f64,
    rss_after_drop: f64,
}

#[allow(clippy::too_many_arguments)]
fn run_cell(
    lib: &std::sync::Arc<Library>,
    base: &Bytes,
    built: &Built,
    chain0: &Chain,
    pages: usize,
    k: usize,
    size: usize,
    work: &Path,
) -> Cell {
    let mut chain = chain0.clone();
    let mut rng = Rng(99 + k as u64);
    let mut secs: Vec<Bytes> = Vec::new();
    for i in 0..k {
        let filler = chain.alloc_id();
        let bytes = chain.section(&[
            (built.content_ids[0], stream_body("", &color_content(i))),
            (filler, filler_body(size, &mut rng)),
        ]);
        let p = work.join(format!("sec{i}.bin"));
        fs::write(&p, &bytes).unwrap();
        secs.push(open_mmap(&p).unwrap());
    }
    let mid = pages / 2;
    let f0 = footprint();
    let reps = 5;
    let (mut opens, mut reopens) = (vec![], vec![]);
    for _ in 0..reps {
        let t = Instant::now();
        let d = Document::open(lib, base.clone(), &secs, None).unwrap();
        opens.push(ms(t.elapsed()));
        drop(d);
    }
    let mut d = Document::open(lib, base.clone(), &secs, None).unwrap();
    for _ in 0..reps {
        let t = Instant::now();
        d.reopen(&secs).unwrap();
        reopens.push(ms(t.elapsed()));
    }
    let rss_open = footprint();
    d.reopen(&secs).unwrap();
    let t = Instant::now();
    d.render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
        .unwrap();
    let tile_ms = ms(t.elapsed());
    let t = Instant::now();
    d.render_tile(mid, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
        .unwrap();
    let tile_mid_ms = ms(t.elapsed());
    let rss_tile = footprint();
    drop(d);
    let rss_after_drop = footprint();
    drop(secs);
    for i in 0..k {
        let _ = fs::remove_file(work.join(format!("sec{i}.bin")));
    }
    Cell {
        open_ms: median(&mut opens),
        reopen_ms: median(&mut reopens),
        tile_ms,
        tile_mid_ms,
        rss_open: mb(rss_open.saturating_sub(f0)),
        rss_tile: mb(rss_tile.saturating_sub(f0)),
        rss_after_drop: mb(rss_after_drop.saturating_sub(f0)),
    }
}

/// 500 edits, each a 1 KB-ish section; L1 merge when k > 16 (latest version of every dirty object).
fn edit_session(
    lib: &std::sync::Arc<Library>,
    base: &Bytes,
    built: &Built,
    chain0: &Chain,
    pages: usize,
    edits: usize,
) {
    let abs0 = footprint();
    let mut d = Document::open(lib, base.clone(), &[], None).unwrap();
    let mut chain = chain0.clone();
    let mut live: Vec<Bytes> = Vec::new();
    let mut dirty: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let (mut lat, mut reo, mut til) = (vec![], vec![], vec![]);
    let (mut rss_warm, mut merges) = (0, 0);
    let mut peak_k = 0;
    for i in 0..edits {
        let pg = (i * 7) % pages;
        let body = stream_body("", &color_content(i));
        dirty.insert(built.content_ids[pg], body.clone());
        let t = Instant::now();
        let sec = chain.section(&[(built.content_ids[pg], body)]);
        live.push(bytes_from_vec(sec));
        if live.len() > 16 {
            chain = chain0.clone();
            let all: Vec<_> = dirty.iter().map(|(k, v)| (*k, v.clone())).collect();
            live = vec![bytes_from_vec(chain.section(&all))];
            merges += 1;
        }
        peak_k = peak_k.max(live.len());
        let t1 = Instant::now();
        d.reopen(&live).unwrap();
        let r = ms(t1.elapsed());
        let t2 = Instant::now();
        let tile = d
            .render_tile(pg, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
            .unwrap();
        let tl = ms(t2.elapsed());
        assert_eq!(tile.pixel(200, 500)[3], 255);
        lat.push(ms(t.elapsed()));
        reo.push(r);
        til.push(tl);
        if i == 49 {
            rss_warm = footprint();
        }
    }
    let end = footprint();
    println!(
        "| pages | edits | L1 merges | peak k | edit→tile ms p50 / p95 / max | reopen ms p50 / p95 | tile ms p50 / p95 | footprint growth after edit 50 MB |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    println!(
        "| {pages} | {edits} | {merges} | {peak_k} | {:.1} / {:.1} / {:.1} | {:.1} / {:.1} | {:.1} / {:.1} | {:.1} |",
        median(&mut lat.clone()),
        pct(&mut lat.clone(), 0.95),
        pct(&mut lat, 1.0),
        median(&mut reo.clone()),
        pct(&mut reo, 0.95),
        median(&mut til.clone()),
        pct(&mut til, 0.95),
        mb(end.saturating_sub(rss_warm)),
    );
    println!(
        "\n(absolute footprint: {:.1} MB before open, {:.1} MB after edit 50, {:.1} MB after last edit)",
        mb(abs0),
        mb(rss_warm),
        mb(end)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let quick = args.iter().any(|a| a == "--quick");
    let dir = args
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| args.get(i + 1))
        .map_or_else(
            || repo_root().join("third_party/cache/render-bench"),
            PathBuf::from,
        );
    fs::create_dir_all(&dir).unwrap();
    if let Some(i) = args.iter().position(|a| a == "--gen") {
        let which = &args[i + 1];
        let (name, pages, lines, rects) = *DOCS.iter().find(|d| d.0 == which).expect("doc name");
        write_doc(&dir.join(format!("{name}.pdf")), pages, lines, rects);
        return;
    }
    let work = dir.join("work");
    fs::create_dir_all(&work).unwrap();

    {
        let f = footprint();
        let mut v = vec![0u8; 64 << 20];
        std::hint::black_box(&mut v);
        v.iter_mut().step_by(4096).for_each(|b| *b = 1);
        println!(
            "Metric self-check: touching a 64 MB heap buffer moves the footprint by {:.1} MB\n",
            mb(footprint().saturating_sub(f))
        );
        std::hint::black_box(&v);
        drop(v);
    }
    let f_before = footprint();
    let lib = Library::load(None).expect("libpdfium (run tools/fetch-pdfium)");
    println!(
        "PDFium {} ({} bytes, {:.2} MB) at {}",
        lib.version().unwrap_or_default(),
        lib.size_bytes(),
        mb(lib.size_bytes()),
        lib.path().display()
    );
    println!(
        "Memory metric: phys_footprint (macOS). Library load: +{:.1} MB\n",
        mb(footprint().saturating_sub(f_before))
    );

    let sizes: [(usize, &str); 5] = [
        (1 << 10, "1 KB"),
        (64 << 10, "64 KB"),
        (1 << 20, "1 MB"),
        (16 << 20, "16 MB"),
        (64 << 20, "64 MB"),
    ];
    let ks = [1usize, 2, 4, 8, 16, 32, 64];
    let session_only = args.iter().any(|a| a == "--session");
    let cap: usize = if quick { 64 << 20 } else { 512 << 20 };

    for (name, pages, _, _) in DOCS {
        let path = dir.join(format!("{name}.pdf"));
        let t = Instant::now();
        if !path.exists() {
            let ok = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--gen", name])
                .arg("--dir")
                .arg(&dir)
                .status()
                .unwrap()
                .success();
            assert!(ok, "document generation failed");
        }
        let (built, chain0) = doc_info(&path, pages);
        let gen_t = t.elapsed();
        let base = open_mmap(&path).unwrap();
        let blen = (*base).as_ref().len();
        println!(
            "## {name}: {pages} pages, {:.1} MB (prepared in {:.1} s)\n",
            mb(blen as u64),
            gen_t.as_secs_f64()
        );

        // Cold open and first tile, base only.
        let f0 = footprint();
        let t = Instant::now();
        let mut d = Document::open(&lib, base.clone(), &[], None).unwrap();
        let open = ms(t.elapsed());
        let t = Instant::now();
        d.render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
            .unwrap();
        let tile = ms(t.elapsed());
        let t = Instant::now();
        let _ = d.page_sizes().unwrap();
        let sizes_ms = ms(t.elapsed());
        let mut cancel_polls = 0u32;
        let t = Instant::now();
        let r = d.render_tile(1 % pages, 0, TileCoord { col: 0, row: 0 }, &mut || {
            cancel_polls += 1;
            true
        });
        let cancel_ms = ms(t.elapsed());
        println!(
            "Base only: open {open:.2} ms, first tile (page 0) {tile:.2} ms, page_sizes() for all pages {sizes_ms:.2} ms, \
             footprint +{:.1} MB; cancel-at-first-poll: {:?} after {cancel_ms:.2} ms ({cancel_polls} poll)\n",
            mb(footprint().saturating_sub(f0)),
            r.err()
        );
        drop(d);

        if !session_only {
            println!(
                "| k sections | section size | open (median of 5) ms | in-place reopen ms | first tile p0 ms | tile p{} cold ms | Δfootprint after open MB | after tile MB | after close MB |",
                pages / 2
            );
            println!("|---|---|---|---|---|---|---|---|---|");
            for &(size, label) in &sizes {
                for &k in &ks {
                    if k * size > cap {
                        println!(
                            "| {k} | {label} | skipped (k × size > {} MB) | | | | | | |",
                            cap >> 20
                        );
                        continue;
                    }
                    let c = run_cell(&lib, &base, &built, &chain0, pages, k, size, &work);
                    println!(
                        "| {k} | {label} | {:.2} | {:.2} | {:.2} | {:.2} | {:.1} | {:.1} | {:.1} |",
                        c.open_ms,
                        c.reopen_ms,
                        c.tile_ms,
                        c.tile_mid_ms,
                        c.rss_open,
                        c.rss_tile,
                        c.rss_after_drop
                    );
                }
            }
        }
        println!();
        println!(
            "Edit session (one ~1 KB section per edit, L1 merge at k > 16, tile of the edited page after every edit):\n"
        );
        edit_session(
            &lib,
            &base,
            &built,
            &chain0,
            pages,
            if quick { 100 } else { 500 },
        );
        println!();
    }
    let _ = fs::remove_dir_all(&work);
    if let Some((fp, peak)) = mem::sample() {
        println!(
            "Process end: footprint {:.0} MB, peak resident {:.0} MB",
            mb(fp),
            mb(peak)
        );
    }
}
