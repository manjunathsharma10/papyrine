//! The renderer role over a real `papyrine-ipc` channel (in-process endpoints).

use papyrine_ipc::*;
use papyrine_render::role::{RendererConfig, Stats, serve};
use papyrine_render::testgen::*;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type C = Client<RenderRequest, RenderResponse>;

struct Harness {
    c: C,
    stats: Arc<Stats>,
    join: Option<JoinHandle<Result<ServeExit, ServeError>>>,
}

impl Harness {
    fn start(cfg: RendererConfig) -> Harness {
        let (host, child) = Endpoint::pair().unwrap();
        let stats: Arc<Stats> = Arc::default();
        let s2 = stats.clone();
        let join = std::thread::spawn(move || serve(child, cfg, s2));
        let c = C::connect(host, &Role::Renderer, Duration::from_secs(10)).unwrap();
        Harness {
            c,
            stats,
            join: Some(join),
        }
    }

    fn call(&self, r: RenderRequest) -> Result<RenderResponse, IpcError> {
        self.c.call_blocking(r)
    }

    fn open(&self, doc: u64, bytes: Vec<u8>) -> u32 {
        match self
            .call(RenderRequest::Open {
                doc: DocId(doc),
                base: DocSource::Bytes(bytes),
                sections: vec![],
                password: None,
            })
            .unwrap()
        {
            RenderResponse::Opened { page_count, .. } => page_count,
            r => panic!("{r:?}"),
        }
    }

    fn tile_inline(&self, doc: u64, page: u32, bucket: i32, x: u32, y: u32) -> PixelsInfo {
        match self
            .call(RenderRequest::RenderTile {
                doc: DocId(doc),
                page,
                bucket,
                tile_x: x,
                tile_y: y,
                dest: PixelDest::Inline,
            })
            .unwrap()
        {
            RenderResponse::Pixels(p) => p,
            r => panic!("{r:?}"),
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.c.shutdown();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn px(p: &PixelsInfo, x: u32, y: u32) -> [u8; 4] {
    let d = p.inline.as_ref().unwrap();
    let i = ((y * p.width + x) * 4) as usize;
    [d[i], d[i + 1], d[i + 2], d[i + 3]]
}

fn text_doc(pages: &[&str]) -> Vec<u8> {
    let specs: Vec<PageSpec> = pages
        .iter()
        .map(|t| {
            PageSpec::letter(
                format!("BT /F1 18 Tf 72 700 Td ({t}) Tj ET\n0 0 0 rg 100 100 50 50 re f\n")
                    .into_bytes(),
            )
        })
        .collect();
    build(&specs).bytes
}

fn quiet() -> RendererConfig {
    RendererConfig {
        warmup: false,
        ..RendererConfig::default()
    }
}

#[test]
fn open_render_inline_and_slot() {
    let h = Harness::start(RendererConfig::default());
    assert_eq!(h.open(1, hello_bytes()), 1);
    let p = h.tile_inline(1, 0, 0, 0, 0);
    assert_eq!((p.width, p.height, p.stride), (512, 512, 2048));
    assert_eq!(px(&p, 200, 500), [0, 0, 0, 255]);
    assert_eq!(px(&p, 10, 10), [255, 255, 255, 255]);

    // Shared-memory pool: 4 slots of 1 MiB.
    let pool = SharedRegion::create(4 << 20).unwrap();
    let mirror = SharedRegion::from_handle(pool.try_clone_handle().unwrap(), 4 << 20).unwrap();
    h.call(RenderRequest::SetTilePool {
        pool,
        slot_bytes: 1 << 20,
        slots: 4,
    })
    .unwrap();
    let r = h
        .call(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(2),
        })
        .unwrap();
    let RenderResponse::Pixels(info) = r else {
        panic!()
    };
    assert_eq!(info.slot, Some(2));
    assert!(info.inline.is_none());
    let bytes = mirror.to_vec(2 * (1 << 20), 512 * 512 * 4);
    let i = (500 * 512 + 200) * 4;
    assert_eq!(&bytes[i..i + 4], &[0, 0, 0, 255]);
    // Bad slot and unknown doc are errors, not crashes.
    let e = h
        .call(RenderRequest::RenderTile {
            doc: DocId(1),
            page: 0,
            bucket: 0,
            tile_x: 0,
            tile_y: 0,
            dest: PixelDest::Slot(9),
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidRequest);
    let e = h
        .call(RenderRequest::PageSizes { doc: DocId(77) })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(Stats::get(&h.stats.warmup_us) > 0);
}

fn hello_bytes() -> Vec<u8> {
    build(&[PageSpec::letter(hello_content())]).bytes
}

#[test]
fn open_errors_map_to_codes() {
    let h = Harness::start(quiet());
    let e = h
        .call(RenderRequest::Open {
            doc: DocId(1),
            base: DocSource::Bytes(b"not a pdf at all".to_vec()),
            sections: vec![],
            password: None,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Corrupt);
}

#[test]
fn page_sizes_with_rotation() {
    let b = build(&[
        PageSpec::letter(hello_content()),
        PageSpec {
            width: 400.0,
            height: 600.0,
            rotate: 90,
            content: hello_content(),
        },
    ]);
    let h = Harness::start(quiet());
    h.open(1, b.bytes);
    let RenderResponse::PageSizes(s) = h.call(RenderRequest::PageSizes { doc: DocId(1) }).unwrap()
    else {
        panic!()
    };
    assert_eq!(s.len(), 2);
    assert_eq!(
        (s[0].width_pt, s[0].height_pt, s[0].rotation),
        (612.0, 792.0, 0)
    );
    assert_eq!(
        (s[1].width_pt, s[1].height_pt, s[1].rotation),
        (600.0, 400.0, 90)
    );
}

#[test]
fn reopen_is_debounced_and_updates_tiles() {
    let b = build(&[PageSpec::letter(hello_content())]);
    let h = Harness::start(RendererConfig {
        warmup: false,
        debounce: Duration::from_millis(80),
        ..RendererConfig::default()
    });
    h.open(1, b.bytes.clone());
    let before = h.tile_inline(1, 0, 0, 0, 0);
    assert_eq!(px(&before, 200, 500), [0, 0, 0, 255]);

    // A burst of five edits: five requests, one re-open, five answers.
    let mut chain = Chain::new(&b);
    let mut pend = vec![];
    let mut sections: Vec<Vec<u8>> = vec![];
    for i in 0..5 {
        let content = if i == 4 {
            red_content()
        } else {
            hello_content()
        };
        sections.push(chain.section(&[(b.content_ids[0], stream_body("", &content))]));
        let srcs: Vec<DocSource> = sections.iter().cloned().map(DocSource::Bytes).collect();
        pend.push(
            h.c.call(RenderRequest::Reopen {
                doc: DocId(1),
                sections: srcs,
            })
            .unwrap(),
        );
    }
    // A tile request flushes the debounce at once (no 80 ms wait) and sees the last edit.
    let t = Instant::now();
    let after = h.tile_inline(1, 0, 0, 0, 0);
    assert!(t.elapsed() < Duration::from_millis(70), "{:?}", t.elapsed());
    assert_eq!(px(&after, 200, 500), [255, 0, 0, 255]);
    for p in pend {
        match p.wait().unwrap() {
            RenderResponse::Opened { page_count, .. } => assert_eq!(page_count, 1),
            r => panic!("{r:?}"),
        }
    }
    assert_eq!(Stats::get(&h.stats.reopens_requested), 5);
    assert_eq!(Stats::get(&h.stats.reopens_applied), 1);

    // With nothing else to do the timer applies it by itself.
    let secs: Vec<DocSource> = sections.iter().cloned().map(DocSource::Bytes).collect();
    let t = Instant::now();
    h.call(RenderRequest::Reopen {
        doc: DocId(1),
        sections: secs,
    })
    .unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(70),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(Stats::get(&h.stats.reopens_applied), 2);
}

#[test]
fn page_text_boxes_and_cache() {
    let h = Harness::start(quiet());
    h.open(1, text_doc(&["Hello world", "second page"]));
    let get = |page| match h
        .call(RenderRequest::PageText {
            doc: DocId(1),
            page,
        })
        .unwrap()
    {
        RenderResponse::Text(t) => t,
        r => panic!("{r:?}"),
    };
    let t = get(0);
    assert!(t.text.starts_with("Hello world"));
    assert_eq!(t.chars.len(), t.text.chars().count());
    let c = t.chars[0];
    assert_eq!(c.ch, 'H');
    assert!(c.right > c.left && c.top > c.bottom);
    assert!((c.left - 72.0).abs() < 1.5, "{c:?}");
    assert_eq!(Stats::get(&h.stats.text_cache_hits), 0);
    get(0);
    assert_eq!(Stats::get(&h.stats.text_cache_hits), 1);
    assert_eq!(get(1).text.trim_end(), "second page");
}

#[test]
fn search_streams_hits_with_first_hit_fast_path() {
    let pages: Vec<String> = (0..60)
        .map(|i| {
            if i % 10 == 3 {
                format!("page {i} has the NEEDLE here and another needle too")
            } else {
                format!("page {i} filler text")
            }
        })
        .collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    let h = Harness::start(quiet());
    h.open(1, text_doc(&refs));
    let job =
        h.c.start_job(RenderRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: "needle".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 0,
            },
        })
        .unwrap();
    let mut got: Vec<SearchHit> = vec![];
    let first = job.partials.recv_timeout(Duration::from_secs(10)).unwrap();
    let RenderResponse::Hits { hits, complete } = first else {
        panic!()
    };
    assert!(!complete);
    assert_eq!(hits.len(), 1, "first hit is sent alone");
    assert_eq!(hits[0].page, 3);
    got.extend(hits);
    let fin = job.wait_timeout(Duration::from_secs(20));
    // Drain what streamed after the first.
    let RenderResponse::Hits { hits, complete } = fin.unwrap().unwrap() else {
        panic!()
    };
    assert!(complete);
    assert!(hits.is_empty(), "streamed searches end with the flag only");
    while let Ok(RenderResponse::Hits { hits, .. }) = job.partials.try_recv() {
        got.extend(hits);
    }
    assert_eq!(got.len(), 12);
    assert_eq!(got.iter().filter(|h| h.page == 3).count(), 2);
    assert!(got[0].context.to_lowercase().contains("needle"));
    assert!(!got[0].quads.is_empty());
    // Quad is in page space near the text baseline (y = 700).
    let q = got[0].quads[0].0;
    assert!(q[1] > 680.0 && q[1] < 720.0, "{q:?}");
}

#[test]
fn search_without_job_returns_everything_and_honours_max_hits() {
    let h = Harness::start(quiet());
    h.open(1, text_doc(&["aa bb aa", "cc aa", "dd"]));
    let q = |max| SearchQuery {
        text: "aa".into(),
        case_sensitive: true,
        whole_word: true,
        max_hits: max,
    };
    let RenderResponse::Hits { hits, complete } = h
        .call(RenderRequest::Search {
            doc: DocId(1),
            query: q(0),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(complete);
    assert_eq!(hits.len(), 3);
    let RenderResponse::Hits { hits, complete } = h
        .call(RenderRequest::Search {
            doc: DocId(1),
            query: q(2),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(!complete);
    assert_eq!(hits.len(), 2);
    let e = h
        .call(RenderRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: "  ".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 0,
            },
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidRequest);
}

#[test]
fn search_can_be_cancelled_and_does_not_block_tiles() {
    let pages: Vec<String> = (0..3000)
        .map(|i| format!("page {i} nothing to see"))
        .collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    let h = Harness::start(quiet());
    h.open(1, text_doc(&refs));
    let job =
        h.c.start_job(RenderRequest::Search {
            doc: DocId(1),
            query: SearchQuery {
                text: "zzzz".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 0,
            },
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    // A visible tile is served while the search is in progress.
    let t = Instant::now();
    let p = h.tile_inline(1, 5, 0, 0, 0);
    assert_eq!(p.width, 512);
    let tile_ms = t.elapsed();
    job.cancel();
    let e = job.wait().unwrap_err();
    assert!(e.is_cancelled(), "{e:?}");
    assert!(
        Stats::get(&h.stats.search_pages) < 3000,
        "search stopped early"
    );
    assert!(tile_ms < Duration::from_millis(500), "{tile_ms:?}");
}

#[test]
fn queued_tile_job_is_cancelled_before_it_runs() {
    let h = Harness::start(quiet());
    h.open(1, hello_bytes());
    // Keep the PDFium thread busy with a long search, queue a job, cancel it.
    let pages: Vec<String> = (0..300).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    h.open(2, text_doc(&refs));
    let busy =
        h.c.start_job(RenderRequest::Search {
            doc: DocId(2),
            query: SearchQuery {
                text: "qqq".into(),
                case_sensitive: false,
                whole_word: false,
                max_hits: 0,
            },
        })
        .unwrap();
    let jobs: Vec<_> = (0..8)
        .map(|_| {
            h.c.start_job(RenderRequest::RenderPreview {
                doc: DocId(1),
                page: 0,
                max_edge: 3000, // large: not a thumbnail
                dest: PixelDest::Inline,
            })
            .unwrap()
        })
        .collect();
    for j in &jobs {
        j.cancel();
    }
    let mut cancelled = 0;
    for j in jobs {
        match j.wait() {
            Err(e) if e.is_cancelled() => cancelled += 1,
            Ok(_) => {}
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(cancelled >= 1);
    busy.cancel();
    let _ = busy.wait();
}

#[test]
fn hibernates_when_idle_and_wakes_on_demand() {
    let mut cfg = quiet();
    cfg.memory.idle_after = Duration::from_millis(60);
    cfg.memory.hibernate_after = Duration::from_millis(120);
    cfg.memory.hibernate_cost = 0;
    let h = Harness::start(cfg);
    h.open(1, hello_bytes());
    h.tile_inline(1, 0, 0, 0, 0);
    let t = Instant::now();
    while Stats::get(&h.stats.hibernations) == 0 && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(Stats::get(&h.stats.hibernations), 1);
    // Sizes come from the cache without waking; a tile wakes it.
    h.call(RenderRequest::PageSizes { doc: DocId(1) }).unwrap();
    let p = h.tile_inline(1, 0, 0, 0, 0);
    assert_eq!(px(&p, 200, 500), [0, 0, 0, 255]);
    assert!(Stats::get(&h.stats.wakes) >= 1);
}

#[test]
fn close_and_shutdown() {
    let h = Harness::start(quiet());
    h.open(1, hello_bytes());
    assert!(matches!(
        h.call(RenderRequest::Close { doc: DocId(1) }).unwrap(),
        RenderResponse::Closed
    ));
    let e = h
        .call(RenderRequest::PageText {
            doc: DocId(1),
            page: 0,
        })
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let RenderResponse::Pong { nonce } = h.call(RenderRequest::Ping { nonce: 9 }).unwrap() else {
        panic!()
    };
    assert_eq!(nonce, 9);
}
