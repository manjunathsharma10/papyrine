//! memprobe: one process, one document, one engine. Driven by `bench/memory/run.py`.
//!
//!   memprobe cos-open  <file> [--no-objects]   Spike 0.3: open + page count + resolve pages
//!                                          (+ object count unless --no-objects)
//!   memprobe pdfium-open <file>            page count oracle for Spike 0.3
//!   memprobe cos       <file> [--path] [--all] [--settle S]   qpdf memory run (default: mmap input)
//!   memprobe cos-lazy  <file> [--iters N]  lazy per-operation fallback: open, one op, drop
//!   memprobe pdfium    <file> [--text] [--reopen-every N] [--settle S]   PDFium memory run
//!   memprobe baseline                      empty process (Rust + nothing loaded)
//!
//! Memory is macOS `phys_footprint` (what Activity Monitor and the section 1.1 gate use):
//! a 10 Hz sampler keeps the maximum, and the kernel's own lifetime peak
//! (`ledger_phys_footprint_peak`) is read at the end. Output is one `MEM {json}` line.
//! File-backed clean pages of the input are not counted by `phys_footprint`.

use papyrine_cos::{Document, Error, OpenOptions};
use papyrine_render as render;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
        min_address: u64,
        max_address: u64,
        ledger_phys_footprint_peak: i64,
        _rest: [u64; 32],
    }
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn task_info(task: u32, flavor: u32, info: *mut TaskVmInfo, count: *mut u32) -> i32;
        fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, goal: usize) -> usize;
        fn malloc_default_zone() -> *mut core::ffi::c_void;
    }
    const TASK_VM_INFO: u32 = 22;

    fn info() -> Option<TaskVmInfo> {
        // SAFETY: plain Mach call into a zeroed, correctly sized out struct.
        unsafe {
            let mut i: TaskVmInfo = std::mem::zeroed();
            let mut count = (std::mem::size_of::<TaskVmInfo>() / 4) as u32;
            (task_info(mach_task_self(), TASK_VM_INFO, &mut i, &mut count) == 0).then_some(i)
        }
    }
    /// Current phys_footprint in bytes.
    pub fn footprint() -> u64 {
        info().map_or(0, |i| i.phys_footprint)
    }
    /// Kernel-tracked lifetime peak of phys_footprint, bytes.
    pub fn footprint_peak() -> u64 {
        info().map_or(0, |i| i.ledger_phys_footprint_peak.max(0) as u64)
    }
    /// (resident, compressed) bytes, for context.
    pub fn resident() -> (u64, u64) {
        info().map_or((0, 0), |i| (i.resident_size, i.compressed))
    }
    /// Ask malloc to return free pages to the OS (what the host could do when idle).
    /// Returns bytes released as reported by malloc (all zones, then the default zone).
    pub fn relief() -> usize {
        // SAFETY: null zone means all zones; malloc_default_zone never returns null.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0)
                + malloc_zone_pressure_relief(malloc_default_zone(), 0)
        }
    }
}

fn mb(b: u64) -> f64 {
    (b as f64 / (1u64 << 20) as f64 * 10.0).round() / 10.0
}

struct Sampler {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    h: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    fn start() -> Sampler {
        let peak = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, s) = (peak.clone(), stop.clone());
        let h = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(mem::footprint(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Sampler {
            peak,
            stop,
            h: Some(h),
        }
    }
    fn peak(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }
    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.h.take() {
            let _ = h.join();
        }
        self.peak()
    }
}

fn arg_val(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn emit(fields: &[(&str, String)]) {
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
    println!("MEM {{{}}}", body.join(","));
}
fn s(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "/").replace('"', "'"))
}

/// Resolve what the engine needs to list and describe a page: the page dict, its content
/// stream object and its resources dict. Returns objects touched.
fn touch_page(doc: &Document, i: usize) -> usize {
    let mut n = 0;
    if let Ok(p) = doc.page(i) {
        n += 1;
        for k in ["Contents", "Resources", "MediaBox"] {
            if let Ok(o) = p.dict_get(k) {
                let _ = o.kind();
                n += 1;
            }
        }
    }
    n
}

fn open_cos(file: &str, use_path: bool) -> Result<(Document, Option<memmap2::Mmap>), Error> {
    if use_path {
        Document::open_path(file, &OpenOptions::default()).map(|d| (d, None))
    } else {
        let f = std::fs::File::open(file).map_err(|e| Error::Io(e.to_string()))?;
        // SAFETY: read-only map of a file nothing else modifies during the run.
        let m = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| Error::Io(e.to_string()))?;
        let _ = &m;
        // The Document owns an Arc over the map so the bytes stay valid.
        let arc = Arc::new(m);
        struct Shared(Arc<memmap2::Mmap>);
        impl AsRef<[u8]> for Shared {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
        Document::open_bytes(Shared(arc), &OpenOptions::default()).map(|d| (d, None))
    }
}

fn cos_open(file: &str, skip_objects: bool) {
    let t0 = Instant::now();
    match Document::open_path(file, &OpenOptions::default()) {
        Ok(doc) => {
            let open_ms = t0.elapsed().as_millis();
            let t1 = Instant::now();
            let pages = doc.page_count();
            let first_ms = t1.elapsed().as_millis();
            let mut bad = 0;
            if let Ok(n) = pages {
                for i in 0..n {
                    if doc.page(i).is_err() {
                        bad += 1;
                    }
                }
            }
            let (status, n) = match pages {
                Ok(0) => ("ok-no-pages", 0),
                Ok(n) => ("ok", n),
                Err(_) => ("ok-no-pages", 0),
            };
            let t2 = Instant::now();
            let objects = if skip_objects {
                "-".into()
            } else {
                doc.object_count().map_or("-".into(), |c| c.to_string())
            };
            println!(
                "RESULT\t{status}\t{n}\t{objects}\t{open_ms}\t{first_ms}\trepairs={} unresolved_pages={bad} objcount_ms={}",
                doc.repair_log().len(),
                t2.elapsed().as_millis()
            );
        }
        Err(e) => {
            let kind = format!("{e:?}");
            let kind = kind.split(['(', ' ', '{']).next().unwrap_or("").to_string();
            println!(
                "RESULT\terr:{kind}\t-\t-\t{}\t0\t{}",
                t0.elapsed().as_millis(),
                e.to_string().replace(['\t', '\n'], " ")
            );
        }
    }
}

/// PDFium as an independent page-count oracle for the Spike 0.3 comparison.
fn pdfium_open(file: &str) {
    let t0 = Instant::now();
    let lib = render::Library::global().expect("libpdfium");
    let opened = render::open_mmap(std::path::Path::new(file))
        .map_err(|e| format!("{e}"))
        .and_then(|b| render::Document::open(&lib, b, &[], None).map_err(|e| format!("{e}")));
    match opened {
        Ok(d) => println!(
            "RESULT\t{}\t{}\t-\t{}\t0\t",
            if d.page_count() > 0 {
                "ok"
            } else {
                "ok-no-pages"
            },
            d.page_count(),
            t0.elapsed().as_millis()
        ),
        Err(e) => println!(
            "RESULT\terr\t-\t-\t{}\t0\t{}",
            t0.elapsed().as_millis(),
            e.replace('\t', " ")
        ),
    }
}

fn run_cos(file: &str, args: &[String]) {
    let use_path = args.iter().any(|a| a == "--path");
    let settle: u64 = arg_val(args, "--settle")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let before = mem::footprint();
    let sampler = Sampler::start();
    let t0 = Instant::now();
    let (doc, _) = open_cos(file, use_path).expect("open");
    let open_ms = t0.elapsed().as_millis();
    let after_open = mem::footprint();
    let t1 = Instant::now();
    let pages = doc.page_count().unwrap_or(0);
    let mut touched = 0;
    for i in 0..pages {
        touched += touch_page(&doc, i);
    }
    let touch_ms = t1.elapsed().as_millis();
    let after_touch = mem::footprint();
    // Worst case, off by default: resolve every object (what a whole-document operation such
    // as Save As or a full rewrite does). `object_count` alone already does this in qpdf.
    let (mut objects, mut all_ms, mut after_all) = (0usize, 0u128, 0u64);
    if args.iter().any(|a| a == "--all") {
        let t = Instant::now();
        if let Ok(ids) = doc.object_ids() {
            objects = ids.len();
            for id in ids {
                if let Ok(o) = doc.object(id) {
                    let _ = o.kind();
                }
            }
        }
        all_ms = t.elapsed().as_millis();
        after_all = mem::footprint();
    }
    std::thread::sleep(Duration::from_secs(settle));
    let settled = mem::footprint();
    mem::relief();
    let settled_relief = mem::footprint();
    let (res, comp) = mem::resident();
    let sampled_peak = sampler.finish();
    let ledger_peak = mem::footprint_peak();
    emit(&[
        ("engine", s("qpdf")),
        ("input", s(if use_path { "path" } else { "mmap" })),
        ("file", s(file)),
        ("pages", pages.to_string()),
        ("objects", objects.to_string()),
        ("touched", touched.to_string()),
        ("open_ms", open_ms.to_string()),
        ("touch_ms", touch_ms.to_string()),
        ("start_mb", mb(before).to_string()),
        ("after_open_mb", mb(after_open).to_string()),
        ("after_touch_mb", mb(after_touch).to_string()),
        ("all_objects_ms", all_ms.to_string()),
        ("after_all_objects_mb", mb(after_all).to_string()),
        ("peak_mb", mb(ledger_peak.max(sampled_peak)).to_string()),
        ("settled_mb", mb(settled).to_string()),
        ("settled_relief_mb", mb(settled_relief).to_string()),
        ("resident_mb", mb(res).to_string()),
        ("compressed_mb", mb(comp).to_string()),
    ]);
    drop(doc);
}

/// Lazy fallback: the engine holds no document between operations. Each "operation" opens
/// the file, resolves one page (the typical view-only query), and drops everything.
fn run_cos_lazy(file: &str, args: &[String]) {
    let iters: usize = arg_val(args, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let start = mem::footprint();
    let sampler = Sampler::start();
    let mut times = Vec::new();
    for k in 0..iters {
        let t = Instant::now();
        let (doc, _) = open_cos(file, false).expect("open");
        let n = doc.page_count().unwrap_or(0);
        if n > 0 {
            touch_page(&doc, (k * 7919) % n);
        }
        times.push(t.elapsed().as_millis());
        drop(doc);
    }
    let after_loop = mem::footprint();
    mem::relief();
    std::thread::sleep(Duration::from_secs(2));
    let settled = mem::footprint();
    let sampled = sampler.finish();
    let peak = mem::footprint_peak().max(sampled);
    let list = times
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",");
    emit(&[
        ("engine", s("qpdf-lazy")),
        ("file", s(file)),
        ("iters", iters.to_string()),
        ("op_ms", format!("[{list}]")),
        ("start_mb", mb(start).to_string()),
        ("after_loop_mb", mb(after_loop).to_string()),
        ("peak_mb", mb(peak).to_string()),
        ("settled_mb", mb(settled).to_string()),
    ]);
}

fn run_pdfium(file: &str, args: &[String]) {
    let settle: u64 = arg_val(args, "--settle")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let with_text = args.iter().any(|a| a == "--text");
    // Mitigation under test: PDFium keeps every parsed object (stream bytes included) of a
    // document until it is closed, so re-open the document every N pages.
    let reopen_every: usize = arg_val(args, "--reopen-every")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let start = mem::footprint();
    let lib = render::Library::global().expect("libpdfium");
    let after_lib = mem::footprint();
    let sampler = Sampler::start();
    let base = render::open_mmap(std::path::Path::new(file)).expect("mmap");
    let t0 = Instant::now();
    let mut doc = render::Document::open(&lib, base, &[], None).expect("open");
    let open_ms = t0.elapsed().as_millis();
    let after_open = mem::footprint();
    let pages = doc.page_count();
    let t1 = Instant::now();
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut cancel = || false;
    for i in 0..pages {
        match doc.render_preview(i, 160, &mut cancel) {
            Ok(_) => ok += 1,
            Err(_) => failed += 1,
        }
        if with_text {
            let _ = doc.page_text(i);
        }
        if reopen_every > 0 && (i + 1) % reopen_every == 0 {
            doc.reopen(&[]).expect("reopen");
        }
    }
    let scan_ms = t1.elapsed().as_millis();
    let after_scan = mem::footprint();
    std::thread::sleep(Duration::from_secs(settle));
    let settled = mem::footprint();
    mem::relief();
    let settled_relief = mem::footprint();
    // What the document itself (not the page cache) holds: close and re-open, then measure.
    let t = Instant::now();
    doc.reopen(&[]).expect("reopen");
    let reopen_ms = t.elapsed().as_millis();
    let released = mem::relief();
    let after_reopen = mem::footprint();
    let sampled = sampler.finish();
    let peak = mem::footprint_peak().max(sampled);
    emit(&[
        ("engine", s("pdfium")),
        ("reopen_every", reopen_every.to_string()),
        ("final_reopen_ms", reopen_ms.to_string()),
        ("after_final_reopen_mb", mb(after_reopen).to_string()),
        ("relief_released_mb", mb(released as u64).to_string()),
        ("file", s(file)),
        ("text", with_text.to_string()),
        ("pages", pages.to_string()),
        ("rendered", ok.to_string()),
        ("render_failed", failed.to_string()),
        ("open_ms", open_ms.to_string()),
        ("scan_ms", scan_ms.to_string()),
        ("start_mb", mb(start).to_string()),
        ("after_lib_mb", mb(after_lib).to_string()),
        ("after_open_mb", mb(after_open).to_string()),
        ("after_scan_mb", mb(after_scan).to_string()),
        ("peak_mb", mb(peak).to_string()),
        ("settled_mb", mb(settled).to_string()),
        ("settled_relief_mb", mb(settled_relief).to_string()),
    ]);
    drop(doc);
}

fn main() {
    main_inner();
    // For attaching vmmap/footprint after the run: MEMPROBE_HOLD=<seconds>.
    if let Some(n) = std::env::var("MEMPROBE_HOLD")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_secs(n));
    }
}

fn main_inner() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let file = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    match mode {
        "cos-open" => cos_open(&file, args.iter().any(|a| a == "--no-objects")),
        "pdfium-open" => pdfium_open(&file),
        "cos" => run_cos(&file, &args),
        "cos-lazy" => run_cos_lazy(&file, &args),
        "pdfium" => run_pdfium(&file, &args),
        "baseline" => emit(&[
            ("engine", s("baseline")),
            ("start_mb", mb(mem::footprint()).to_string()),
        ]),
        _ => {
            eprintln!("usage: memprobe cos-open|cos|cos-lazy|pdfium <file> | baseline");
            std::process::exit(2);
        }
    }
}
