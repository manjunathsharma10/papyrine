//! The ARCHITECTURE section 1.2 large-document script against the real renderer
//! role in a real (sandboxed) child process:
//!
//!   open -> full scroll -> whole-document search -> 50 section re-opens -> idle
//!
//! The harness process plays the host and the engine (it opens the document
//! with qpdf, as the engine would); the renderer is the `render_role` example
//! binary. Both are sampled at 10 Hz. The webview/shell and the host's tile
//! caches are not running here: their share is a documented constant
//! (`--shell-mb`, `--broker-mb`) added to the sums, and reported separately so
//! nothing is hidden. Output: one `GATE {json}` line.

use papyrine_cos::{Document as Cos, OpenOptions};
use papyrine_ipc::{
    ChildClient, ChildSpec, DocId, DocSource, Handle, PixelDest, RenderRequest, RenderResponse,
    Role, SearchQuery, SharedRegion,
};
use papyrine_render::testgen::{Chain, stream_body};
use papyrine_render::{mem as rmem, tiles};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type CC = ChildClient<RenderRequest, RenderResponse>;

mod proc {
    pub fn footprint(pid: u32) -> Option<(u64, u64)> {
        papyrine_render::mem::footprint_of(pid)
    }
}

const MB: f64 = 1024.0 * 1024.0;

fn mb(b: u64) -> f64 {
    (b as f64 / MB * 10.0).round() / 10.0
}

#[derive(Clone, Copy)]
struct Sample {
    t: f64,
    engine: u64,
    renderer: u64,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1]
}

fn arg_val(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Offset of the last `startxref`, read from the file tail.
fn startxref_of(file: &Path) -> usize {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).expect("open");
    let len = f.metadata().unwrap().len();
    let n = len.min(4096);
    f.seek(SeekFrom::Start(len - n)).unwrap();
    let mut tail = vec![0u8; n as usize];
    f.read_exact(&mut tail).unwrap();
    let t = String::from_utf8_lossy(&tail).into_owned();
    let i = t.rfind("startxref").expect("startxref") + "startxref".len();
    t[i..]
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

pub fn run(file: &str, args: &[String]) {
    let settle: u64 = arg_val(args, "--settle")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let tiles_per_page: usize = arg_val(args, "--tiles")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let bucket: i32 = arg_val(args, "--bucket")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let edits: usize = arg_val(args, "--edits")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let page_limit: usize = arg_val(args, "--pages")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let shell_mb: f64 = arg_val(args, "--shell-mb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(77.4);
    // Host broker caches (ADR-009): L1 tiles soft cap 48 MB, previews 16 MB (QOI, about a
    // third of raw), trimmed to the visible set when idle. Estimated from the bytes delivered
    // unless overridden.
    let broker_override: Option<f64> = arg_val(args, "--broker-mb").and_then(|v| v.parse().ok());
    let broker_settled_mb: f64 = arg_val(args, "--broker-settled-mb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8.0);
    let word = arg_val(args, "--search").unwrap_or_else(|| "zzqxj".into());
    let role_exe = PathBuf::from(arg_val(args, "--role-exe").expect("--role-exe PATH"));
    let sandbox = !args.iter().any(|a| a == "--no-sandbox");
    let alloc_env = !args.iter().any(|a| a == "--no-alloc-env");
    let path = PathBuf::from(file);

    // --- spawn the renderer
    let mut spec = ChildSpec::current_exe(Role::Renderer).unwrap();
    spec.exe = role_exe;
    if let Some(d) = std::env::var_os("PAPYRINE_PDFIUM_DIR") {
        spec = spec.read_dir(PathBuf::from(d));
    }
    if !sandbox {
        spec = spec.unsandboxed();
    }
    if alloc_env {
        for (k, v) in rmem::CHILD_ENV {
            spec = spec.env(*k, *v);
        }
    }
    let t_start = Instant::now();
    let cc: CC = ChildClient::spawn(&spec, Duration::from_secs(60)).expect("spawn renderer");
    let spawn_ms = t_start.elapsed().as_millis();
    let pid = cc.process.id();
    let client = cc.client.clone();

    // --- sampler (10 Hz, both processes)
    let samples: Arc<Mutex<Vec<Sample>>> = Arc::default();
    let phases: Arc<Mutex<Vec<(f64, &'static str)>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    let sampler = {
        let (samples, stop) = (samples.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let r = proc::footprint(pid).map_or(0, |x| x.0);
                samples.lock().unwrap().push(Sample {
                    t: t0.elapsed().as_secs_f64(),
                    engine: rmem::footprint(),
                    renderer: r,
                });
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let mark = |name: &'static str| {
        phases
            .lock()
            .unwrap()
            .push((t0.elapsed().as_secs_f64(), name))
    };
    let engine_base = rmem::footprint();

    // --- tile pool
    let pool = SharedRegion::create(8 << 20).unwrap();
    client
        .call_blocking(RenderRequest::SetTilePool {
            pool,
            slot_bytes: 1 << 20,
            slots: 8,
        })
        .unwrap();

    // --- 1. open: engine (qpdf) and renderer in parallel, as the app does
    mark("open");
    let t_open = Instant::now();
    let eng_path = path.clone();
    let (eng_tx, eng_rx) = std::sync::mpsc::channel::<(u128, Option<(u32, u32, u32)>)>();
    let (eng_stop_tx, eng_stop_rx) = std::sync::mpsc::channel::<()>();
    let eng = std::thread::spawn(move || {
        let t = Instant::now();
        let f = std::fs::File::open(&eng_path).unwrap();
        // SAFETY: read-only map of a file nothing modifies during the run.
        let m = unsafe { memmap2::Mmap::map(&f) }.unwrap();
        struct Shared(Arc<memmap2::Mmap>);
        impl AsRef<[u8]> for Shared {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
        let doc = Cos::open_bytes(Shared(Arc::new(m)), &OpenOptions::default()).expect("qpdf open");
        let n = doc.page_count().unwrap_or(0);
        for i in 0..n.min(2000) {
            if let Ok(p) = doc.page(i) {
                for k in ["Contents", "Resources", "MediaBox"] {
                    if let Ok(o) = p.dict_get(k) {
                        let _ = o.kind();
                    }
                }
            }
        }
        // The content stream the edits will replace.
        // Content stream id plus /Size and /Root of the trailer for the edit chain.
        let info = (|| {
            let c = doc.page(0).ok()?.dict_get("Contents").ok()?;
            let cid = c.id().or_else(|| c.array_get(0).ok()?.id())?.num;
            let tr = doc.trailer().ok()?;
            let size = tr.dict_get("Size").ok()?.as_int().ok()? as u32;
            let root = tr.dict_get("Root").ok()?.id()?.num;
            Some((cid, size, root))
        })();
        let _ = eng_tx.send((t.elapsed().as_millis(), info));
        // The engine keeps its document open for the whole run.
        let _ = eng_stop_rx.recv();
        drop(doc);
    });
    let handle = Handle::from_file(std::fs::File::open(&path).unwrap());
    let len = std::fs::metadata(&path).unwrap().len();
    let r = client
        .call_blocking(RenderRequest::Open {
            doc: DocId(1),
            base: DocSource::File { handle, len },
            sections: vec![],
            password: None,
        })
        .expect("renderer open");
    let RenderResponse::Opened { page_count, .. } = r else {
        panic!("{r:?}")
    };
    let renderer_open_ms = t_open.elapsed().as_millis();
    let (engine_open_ms, edit_info) = eng_rx.recv().unwrap();
    let pages = (page_count as usize).min(page_limit);

    // First page latency (the < 500 ms / < 2 s criterion), after open.
    let t = Instant::now();
    let first = client
        .call_blocking(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(0),
        })
        .expect("first tile");
    assert!(matches!(first, RenderResponse::Pixels(_)));
    let first_tile_ms = t.elapsed().as_secs_f64() * 1000.0;

    // --- 2. full scroll: per page one preview and `tiles_per_page` visible tiles
    mark("scroll");
    let t_scroll = Instant::now();
    let mut tile_ms: Vec<f64> = Vec::new();
    let mut failed = 0usize;
    let mut slot = 0u32;
    let (mut tile_bytes, mut preview_bytes) = (0u64, 0u64);
    for p in 0..pages {
        let mut pend = Steps(Vec::new());
        let t = Instant::now();
        pend.push(
            client
                .call(RenderRequest::RenderPreview {
                    doc: DocId(1),
                    page: p as u32,
                    max_edge: 500,
                    dest: PixelDest::Slot({
                        slot = (slot + 1) % 8;
                        slot
                    }),
                })
                .unwrap(),
        );
        // Visible tiles: the top-left block of the page grid at `bucket`.
        for k in 0..tiles_per_page {
            let (col, row) = ((k % 2) as u32, (k / 2) as u32);
            let job = client
                .start_job(RenderRequest::RenderTile {
                    doc: DocId(1),
                    page: p as u32,
                    bucket,
                    tile_x: col,
                    tile_y: row,
                    dest: PixelDest::Slot({
                        slot = (slot + 1) % 8;
                        slot
                    }),
                })
                .unwrap();
            pend.push_job(job);
        }
        for (i, r) in pend.finish().into_iter().enumerate() {
            match r {
                Ok(RenderResponse::Pixels(px)) => {
                    let b = px.width as u64 * px.height as u64 * 4;
                    if i == 0 {
                        preview_bytes += b;
                    } else {
                        tile_bytes += b;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if failed == 0 {
                        eprintln!("first render failure (page {p}): {e}");
                    }
                    failed += 1;
                }
            }
        }
        tile_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let scroll_s = t_scroll.elapsed().as_secs_f64();

    // --- 3. whole-document search (streams; first hit is the fast path)
    mark("search");
    let t_search = Instant::now();
    let job = client
        .start_job(RenderRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: word.clone(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 0,
            },
        })
        .unwrap();
    let mut first_hit_ms = None;
    let mut hits = 0usize;
    let mut done = None;
    while done.is_none() {
        if let Ok(RenderResponse::Hits { hits: h, .. }) =
            job.partials.recv_timeout(Duration::from_millis(20))
        {
            first_hit_ms.get_or_insert(t_search.elapsed().as_secs_f64() * 1000.0);
            hits += h.len();
        }
        done = job.wait_timeout(Duration::ZERO);
    }
    while let Ok(RenderResponse::Hits { hits: h, .. }) = job.partials.try_recv() {
        hits += h.len();
    }
    let search_s = t_search.elapsed().as_secs_f64();
    let search_ok = matches!(done, Some(Ok(RenderResponse::Hits { .. })));

    // --- 4. edits: replace page 0's content, 50 times; L1 merge when k > 16
    mark("edits");
    let mut edit_ms: Vec<f64> = Vec::new();
    let mut edits_done = 0;
    if let Some((cid, size, root)) = edit_info {
        let startxref = startxref_of(&path);
        let base = papyrine_render::testgen::Built {
            bytes: Vec::new(),
            startxref,
            size,
            root,
            page_ids: vec![],
            content_ids: vec![cid],
        };
        let base_len = len as usize;
        let fresh = || {
            let mut c = Chain::new(&base);
            c.total_len = base_len;
            c
        };
        let mut chain = fresh();
        let mut secs: Vec<Vec<u8>> = Vec::new();
        for i in 0..edits {
            let content = format!(
                "{:.2} 0.2 0.8 rg 40 40 300 300 re f\n",
                (i % 10) as f32 / 10.0
            );
            let body = stream_body("", content.as_bytes());
            if secs.len() >= 16 {
                // L1 merge: one section with the latest version of each dirty object.
                chain = fresh();
                secs.clear();
            }
            secs.push(chain.section(&[(cid, body)]));
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
            let pending = client
                .call(RenderRequest::Reopen {
                    doc: DocId(1),
                    sections: srcs,
                })
                .unwrap();
            let r = client.call_blocking(RenderRequest::RenderTile {
                doc: DocId(1),
                page: 0,
                bucket: 0,
                tile_x: 0,
                tile_y: 0,
                dest: PixelDest::Slot(0),
            });
            let ok = r.is_ok() && pending.wait().is_ok();
            if ok {
                edit_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                edits_done += 1;
            }
            std::thread::sleep(Duration::from_millis(20)); // the user is not a loop
        }
    }

    // --- 5. idle
    mark("idle");
    let t_idle = Instant::now();
    std::thread::sleep(Duration::from_secs(settle));
    let idle_s = t_idle.elapsed().as_secs_f64();
    mark("end");

    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    let samples = samples.lock().unwrap().clone();
    let phases = phases.lock().unwrap().clone();

    // --- numbers
    let (cur, lifetime_peak) = proc::footprint(pid).unwrap_or((0, 0));
    let ren_peak_sampled = samples.iter().map(|s| s.renderer).max().unwrap_or(0);
    let ren_peak = ren_peak_sampled.max(lifetime_peak);
    let eng_peak = samples.iter().map(|s| s.engine).max().unwrap_or(0);
    let idle_start = phases.iter().find(|p| p.1 == "idle").map_or(0.0, |p| p.0);
    // Settled: the median of the last 2 s of the idle window.
    let tail: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.t >= idle_start + idle_s - 2.0)
        .collect();
    let med = |f: fn(&Sample) -> u64| -> u64 {
        let mut v: Vec<u64> = tail.iter().map(|s| f(s)).collect();
        v.sort_unstable();
        v.get(v.len() / 2).copied().unwrap_or(0)
    };
    let (ren_settled, eng_settled) = (med(|s| s.renderer), med(|s| s.engine));
    let sim_peak = samples
        .iter()
        .map(|s| s.engine + s.renderer)
        .max()
        .unwrap_or(0);
    let shell = (shell_mb * MB) as u64;
    let broker_mb = broker_override.unwrap_or_else(|| {
        (tile_bytes as f64 / MB).min(48.0) + (preview_bytes as f64 / MB / 3.0).min(16.0)
    });
    let broker = (broker_mb * MB) as u64;
    let broker_settled = (broker_settled_mb.min(broker_mb) * MB) as u64;
    let peak_total = sim_peak + shell + broker;
    let settled_total = ren_settled + eng_settled + shell + broker_settled;
    let peak_bound = ren_peak + eng_peak + shell + broker;
    // Per-phase renderer peak.
    let phase_peak = |from: &str, to: &str| -> u64 {
        let a = phases.iter().find(|p| p.1 == from).map_or(0.0, |p| p.0);
        let b = phases.iter().find(|p| p.1 == to).map_or(f64::MAX, |p| p.0);
        samples
            .iter()
            .filter(|s| s.t >= a && s.t < b)
            .map(|s| s.renderer)
            .max()
            .unwrap_or(0)
    };
    let mut tm = tile_ms.clone();
    let mut em = edit_ms.clone();
    let _ = tiles::TILE_SIZE;
    let _ = eng_stop_tx.send(());
    let _ = eng.join();
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    println!(
        "GATE {}",
        format_args!(
            "{{\"file\":\"{name}\",\"pages\":{page_count},\"scrolled\":{pages},\"sandbox\":{sandbox},\"alloc_env\":{alloc_env},\
\"spawn_ms\":{spawn_ms},\"renderer_open_ms\":{renderer_open_ms},\"engine_open_ms\":{engine_open_ms},\"first_tile_ms\":{first_tile_ms:.1},\
\"scroll_s\":{scroll_s:.1},\"page_ms_p50\":{:.1},\"page_ms_p95\":{:.1},\"render_failed\":{failed},\
\"search_s\":{search_s:.2},\"search_first_hit_ms\":{},\"search_hits\":{hits},\"search_ok\":{search_ok},\
\"edits\":{edits_done},\"edit_tile_ms_p50\":{:.1},\"edit_tile_ms_p95\":{:.1},\
\"renderer_peak_mb\":{},\"renderer_peak_open_mb\":{},\"renderer_peak_scroll_mb\":{},\"renderer_peak_search_mb\":{},\"renderer_peak_edits_mb\":{},\
\"renderer_settled_mb\":{},\"renderer_end_mb\":{},\
\"engine_base_mb\":{},\"engine_peak_mb\":{},\"engine_settled_mb\":{},\
\"sim_peak_er_mb\":{},\"shell_mb\":{shell_mb},\"broker_mb\":{broker_mb:.1},\"broker_settled_mb\":{broker_settled_mb},\
\"peak_mb\":{},\"peak_bound_mb\":{},\"settled_mb\":{}}}",
            pct(&mut tm, 0.5),
            pct(&mut tm, 0.95),
            first_hit_ms.map_or("null".into(), |v| format!("{v:.1}")),
            pct(&mut em, 0.5),
            pct(&mut em, 0.95),
            mb(ren_peak),
            mb(phase_peak("open", "scroll")),
            mb(phase_peak("scroll", "search")),
            mb(phase_peak("search", "edits")),
            mb(phase_peak("edits", "idle")),
            mb(ren_settled),
            mb(cur),
            mb(engine_base),
            mb(eng_peak),
            mb(eng_settled),
            mb(sim_peak),
            mb(peak_total),
            mb(peak_bound),
            mb(settled_total),
        )
    );
    let _ = cc.shutdown(Duration::from_secs(10));
}

/// Pipelined requests of one scroll step.
enum Step {
    Plain(papyrine_ipc::Pending<RenderResponse>),
    Job(papyrine_ipc::Job<RenderRequest, RenderResponse>),
}

struct Steps(Vec<Step>);

impl Steps {
    fn push(&mut self, p: papyrine_ipc::Pending<RenderResponse>) {
        self.0.push(Step::Plain(p));
    }
    fn push_job(&mut self, j: papyrine_ipc::Job<RenderRequest, RenderResponse>) {
        self.0.push(Step::Job(j));
    }
    fn finish(self) -> Vec<Result<RenderResponse, papyrine_ipc::IpcError>> {
        self.0
            .into_iter()
            .map(|s| match s {
                Step::Plain(p) => p.wait(),
                Step::Job(j) => j.wait(),
            })
            .collect()
    }
}
