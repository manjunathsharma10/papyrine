//! The renderer role: a server over `papyrine-ipc` (ARCHITECTURE §1.2, §4.6, §4.7).
//!
//! Two threads, as §6 prescribes: the caller's thread is **the one PDFium
//! thread**; an IPC thread reads frames, classifies them into a priority
//! queue ([`crate::sched`]) and cancels jobs the moment `Cancel` arrives. The
//! PDFium thread runs one item at a time; long work (search) runs in short
//! slices that go back to the queue, so a visible tile never waits behind it.
//!
//! Documents are opened from the host's mapping (`DocSource`) plus snapshot
//! sections, never copied. A `Reopen` is debounced (~50 ms of quiet) and is
//! applied early when a request for that document arrives. Memory is governed
//! by [`crate::governor`]: pages closed promptly, documents recycled, and
//! expensive ones hibernated when idle.
//!
//! Wire priorities. [`RenderRequest`] has no priority field, so it is
//! inferred ([`classify`]): control messages first; `RenderTile` carrying a job
//! id is a *visible* tile, without one a *prefetch* tile; `RenderPreview` up to
//! 256 px is a thumbnail, larger a preview; `PageText` ranks with previews;
//! `Search` is last and streams `Partial` hits when it has a job id.

use crate::document::{Document, Glyphs};
use crate::error::Error;
use crate::governor::{ColdLoad, Governor, MemoryConfig};
use crate::library::Library;
use crate::sched::{Item, Popped, Priority, Queue};
use crate::source::Bytes;
use crate::tiles::TileCoord;
use crate::{mem, testgen};
use papyrine_core::{CancelToken, Rect};
use papyrine_ipc::{
    ChildToHost, DocId, DocSource, Endpoint, ErrorCode, Hello, HostToChild, IpcError, JobId,
    PROTOCOL_MIN, PROTOCOL_VERSION, PageSizeInfo, PageTextInfo, PixelDest, PixelsInfo, Quad,
    Receiver, RenderRequest, RenderResponse, RequestId, Role, SearchHit as WireHit, SearchQuery,
    Sender, ServeError, ServeExit, SharedRegion, TransportError, negotiate,
};
use papyrine_text::{PageChars, PageInput, SearchOptions, Searcher};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Largest preview edge the renderer accepts (a 4096 x 4096 bitmap is 64 MB).
pub const MAX_PREVIEW_EDGE: u32 = 4096;

#[derive(Clone, Debug)]
pub struct RendererConfig {
    pub memory: MemoryConfig,
    /// Quiet time before a `Reopen` is applied (if no request needs it sooner).
    pub debounce: Duration,
    /// Render a tiny built-in page once, right after the handshake, so PDFium's
    /// one-time font initialisation is not paid by the first real tile.
    pub warmup: bool,
    pub pdfium_dir: Option<PathBuf>,
    /// A slice of a search runs at most this long before it yields to the queue.
    pub search_slice: Duration,
}

impl Default for RendererConfig {
    fn default() -> Self {
        RendererConfig {
            memory: MemoryConfig::default(),
            debounce: Duration::from_millis(50),
            warmup: true,
            pdfium_dir: None,
            search_slice: Duration::from_millis(15),
        }
    }
}

/// Counters, readable from another thread (tests, diagnostics).
#[derive(Default, Debug)]
pub struct Stats {
    pub tiles: AtomicU64,
    pub previews: AtomicU64,
    pub text_requests: AtomicU64,
    pub text_cache_hits: AtomicU64,
    pub reopens_requested: AtomicU64,
    pub reopens_applied: AtomicU64,
    pub recycles: AtomicU64,
    pub hibernations: AtomicU64,
    pub wakes: AtomicU64,
    pub trims: AtomicU64,
    pub preempts: AtomicU64,
    pub search_pages: AtomicU64,
    pub warmup_us: AtomicU64,
    /// Footprint after the warm-up (the process's own floor), bytes.
    pub floor_bytes: AtomicU64,
}

impl Stats {
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}

fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// The priority a request gets (see the module docs).
pub fn classify(req: &RenderRequest, job: Option<JobId>) -> Priority {
    match req {
        RenderRequest::RenderTile { .. } => {
            if job.is_some() {
                Priority::Visible
            } else {
                Priority::Prefetch
            }
        }
        RenderRequest::RenderPreview { max_edge, .. } => {
            if *max_edge <= 256 {
                Priority::Thumbnail
            } else {
                Priority::Preview
            }
        }
        RenderRequest::PageText { .. } => Priority::Preview,
        RenderRequest::Search { .. } => Priority::Search,
        _ => Priority::Control,
    }
}

enum Work {
    Req { req: RenderRequest, tries: u8 },
    Search(Box<SearchTask>),
}

enum Outcome {
    Done(Result<RenderResponse, IpcError>),
    /// Put the work back (preempted or a search slice ended).
    Requeue(Work),
    /// The answer is sent later (debounced re-open).
    Deferred,
}

struct Pool {
    region: SharedRegion,
    slot_bytes: usize,
    slots: u32,
}

struct PendingReopen {
    sections: Vec<Bytes>,
    due: Instant,
    replies: Vec<RequestId>,
}

struct DocEntry {
    base: Bytes,
    sections: Vec<Bytes>,
    password: Option<String>,
    doc: Option<Document>,
    page_count: u32,
    /// What the open document costs by itself (parse state), bytes.
    open_cost: u64,
    sizes: Option<Vec<PageSizeInfo>>,
    pending: Option<PendingReopen>,
}

struct TextCache {
    map: HashMap<(DocId, u32), (Arc<Glyphs>, u64)>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl TextCache {
    fn new(cap: usize) -> Self {
        TextCache {
            map: HashMap::new(),
            bytes: 0,
            cap,
            tick: 0,
        }
    }

    fn get(&mut self, k: (DocId, u32)) -> Option<Arc<Glyphs>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }

    fn peek(&self, k: (DocId, u32)) -> Option<Arc<Glyphs>> {
        self.map.get(&k).map(|e| e.0.clone())
    }

    fn put(&mut self, k: (DocId, u32), g: Arc<Glyphs>) {
        let sz = g.heap_bytes();
        if sz > self.cap {
            return;
        }
        self.tick += 1;
        if let Some(old) = self.map.insert(k, (g, self.tick)) {
            self.bytes -= old.0.heap_bytes();
        }
        self.bytes += sz;
        while self.bytes > self.cap {
            let Some(victim) = self.map.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| *k) else {
                break;
            };
            if let Some(v) = self.map.remove(&victim) {
                self.bytes -= v.0.heap_bytes();
            }
        }
    }

    fn invalidate_doc(&mut self, doc: DocId) {
        self.map.retain(|k, v| {
            if k.0 == doc {
                self.bytes -= v.0.heap_bytes();
                false
            } else {
                true
            }
        });
    }
}

struct SearchTask {
    doc: DocId,
    searcher: Searcher,
    next: usize,
    hits: usize,
    max_hits: usize,
    streaming: bool,
    batch: Vec<WireHit>,
    all: Vec<WireHit>,
    first_sent: bool,
    last_flush: Instant,
    truncated: bool,
}

struct Server {
    lib: Result<Arc<Library>, String>,
    cfg: RendererConfig,
    docs: HashMap<DocId, DocEntry>,
    pool: Option<Pool>,
    scratch: Vec<u8>,
    text: TextCache,
    gov: Governor,
    tx: Arc<Sender>,
    q: Arc<Queue<Work>>,
    stats: Arc<Stats>,
    floor: u64,
    active: Option<(DocId, usize)>,
    last_activity: Instant,
    idle_stage: u8,
}

fn map_err(e: Error) -> IpcError {
    let code = match &e {
        Error::PasswordRequired | Error::PasswordIncorrect | Error::Security => {
            ErrorCode::Encrypted
        }
        Error::Format => ErrorCode::Corrupt,
        Error::File | Error::PageLoad(_) => ErrorCode::Io,
        Error::TooLarge => ErrorCode::Unsupported,
        Error::PageOutOfRange(_) => ErrorCode::InvalidRequest,
        Error::Cancelled => ErrorCode::Cancelled,
        Error::Library(_) | Error::Render(_) | Error::Other(_) => ErrorCode::Internal,
    };
    IpcError::new(code, e.to_string())
}

fn unknown_doc(d: DocId) -> IpcError {
    IpcError::new(ErrorCode::NotFound, format!("document {} is not open", d.0))
}

fn sources(list: Vec<DocSource>) -> Result<Vec<Bytes>, IpcError> {
    list.into_iter()
        .map(|s| s.into_bytes().map(|b| b as Bytes))
        .collect()
}

impl Server {
    fn send(&self, id: RequestId, result: Result<RenderResponse, IpcError>) {
        let _ = self
            .tx
            .send(&ChildToHost::<RenderResponse>::Response { id, result });
    }

    fn growth(&self) -> u64 {
        let held: u64 = self
            .docs
            .values()
            .filter(|e| e.doc.is_some())
            .map(|e| e.open_cost)
            .sum();
        mem::footprint().saturating_sub(self.floor + held)
    }

    fn lib(&self) -> Result<Arc<Library>, IpcError> {
        self.lib
            .clone()
            .map_err(|e| IpcError::new(ErrorCode::Internal, e))
    }

    fn open_entry(
        &mut self,
        base: Bytes,
        sections: Vec<Bytes>,
        password: Option<String>,
    ) -> Result<DocEntry, IpcError> {
        let lib = self.lib()?;
        let before = mem::footprint();
        let doc =
            Document::open(&lib, base.clone(), &sections, password.as_deref()).map_err(map_err)?;
        let cost = mem::footprint().saturating_sub(before);
        Ok(DocEntry {
            base,
            sections,
            password,
            page_count: doc.page_count() as u32,
            doc: Some(doc),
            open_cost: cost,
            sizes: None,
            pending: None,
        })
    }

    /// Apply a pending re-open now and answer everyone waiting for it.
    fn apply_pending(&mut self, id: DocId) {
        let Some(p) = self.docs.get_mut(&id).and_then(|e| e.pending.take()) else {
            return;
        };
        let lib = self.lib();
        let heavy_at = self.cfg.memory.hibernate_cost;
        let result: Result<u32, IpcError> = (|| {
            let e = self.docs.get_mut(&id).ok_or_else(|| unknown_doc(id))?;
            let heavy = e.open_cost >= heavy_at;
            match e.doc.as_mut() {
                Some(d) if !heavy => d.reopen(&p.sections).map_err(map_err)?,
                _ => {
                    // Hibernated, or a document whose parse state alone is large: close
                    // it first so two copies never coexist (peak stays at one copy).
                    let lib = lib?;
                    if e.doc.take().is_some() {
                        mem::relief();
                    }
                    match Document::open(&lib, e.base.clone(), &p.sections, e.password.as_deref()) {
                        Ok(d) => e.doc = Some(d),
                        Err(err) => {
                            // Put the previous snapshot back; the caller sees the error.
                            e.doc = Document::open(
                                &lib,
                                e.base.clone(),
                                &e.sections,
                                e.password.as_deref(),
                            )
                            .ok();
                            return Err(map_err(err));
                        }
                    }
                    bump(&self.stats.wakes);
                }
            }
            e.sections = p.sections.clone();
            e.sizes = None;
            e.page_count = e.doc.as_ref().map_or(0, |d| d.page_count() as u32);
            Ok(e.page_count)
        })();
        if result.is_ok() {
            self.text.invalidate_doc(id);
            bump(&self.stats.reopens_applied);
        }
        for r in p.replies {
            self.send(
                r,
                result.clone().map(|page_count| RenderResponse::Opened {
                    doc: id,
                    page_count,
                }),
            );
        }
    }

    /// Everything that must hold before touching `doc` (and `page`, if given):
    /// flush a pending re-open, wake a hibernated document, apply the memory policy.
    fn prepare(&mut self, id: DocId, page: Option<usize>) -> Result<(), IpcError> {
        if !self.docs.contains_key(&id) {
            return Err(unknown_doc(id));
        }
        self.apply_pending(id);
        if self.docs.get(&id).is_some_and(|e| e.doc.is_none()) {
            let lib = self.lib()?;
            let e = self.docs.get_mut(&id).ok_or_else(|| unknown_doc(id))?;
            let before = mem::footprint();
            let d = Document::open(&lib, e.base.clone(), &e.sections, e.password.as_deref())
                .map_err(map_err)?;
            e.open_cost = mem::footprint().saturating_sub(before);
            e.doc = Some(d);
            bump(&self.stats.wakes);
        }
        if let Some(p) = page {
            let cached = self
                .docs
                .get(&id)
                .and_then(|e| e.doc.as_ref())
                .is_some_and(|d| d.is_page_cached(p));
            if !cached {
                let loads = self
                    .docs
                    .get(&id)
                    .and_then(|e| e.doc.as_ref())
                    .map_or(0, |d| d.pages_loaded());
                let growth = self.growth();
                match self.gov.before_cold_load(growth, loads) {
                    ColdLoad::Proceed => {}
                    ColdLoad::TrimPages => self.trim_pages(self.active.filter(|(d, _)| *d == id)),
                    ColdLoad::Recycle => {
                        self.trim_pages(None);
                        self.recycle(id, growth)?;
                    }
                }
            }
        }
        self.active = page.map(|p| (id, p)).or(self.active);
        Ok(())
    }

    /// Close cached pages everywhere, keeping `keep` (doc, page) if given.
    fn trim_pages(&mut self, keep: Option<(DocId, usize)>) {
        bump(&self.stats.trims);
        for (id, e) in self.docs.iter_mut() {
            if let Some(d) = e.doc.as_mut() {
                match keep {
                    Some((kd, kp)) if kd == *id => d.close_pages_except(kp),
                    _ => d.close_pages(),
                }
            }
        }
    }

    fn recycle(&mut self, id: DocId, growth_before: u64) -> Result<(), IpcError> {
        let e = self.docs.get_mut(&id).ok_or_else(|| unknown_doc(id))?;
        if let Some(d) = e.doc.as_mut() {
            d.reopen(&e.sections).map_err(map_err)?;
            bump(&self.stats.recycles);
            mem::relief();
            let after = self.growth();
            self.gov.recycled(growth_before, after);
        }
        Ok(())
    }

    fn doc_mut(&mut self, id: DocId) -> Result<&mut Document, IpcError> {
        self.docs
            .get_mut(&id)
            .and_then(|e| e.doc.as_mut())
            .ok_or_else(|| unknown_doc(id))
    }

    fn after_job(&mut self) {
        self.last_activity = Instant::now();
        self.idle_stage = 0;
        let growth = self.growth();
        if self.gov.trim_after_job(growth) {
            self.trim_pages(self.active);
        }
    }

    // ------------------------------------------------------------ requests

    fn handle(
        &mut self,
        prio: Priority,
        cancel: &CancelToken,
        req: &RenderRequest,
        tries: u8,
    ) -> Outcome {
        let r: Result<RenderResponse, IpcError> = (|| match req {
            RenderRequest::Ping { nonce } => Ok(RenderResponse::Pong { nonce: *nonce }),
            RenderRequest::Close { doc } => {
                if let Some(e) = self.docs.remove(doc) {
                    for r in e.pending.map(|p| p.replies).unwrap_or_default() {
                        self.send(r, Err(IpcError::invalid("document closed")));
                    }
                }
                self.text.invalidate_doc(*doc);
                if self.active.is_some_and(|(d, _)| d == *doc) {
                    self.active = None;
                }
                mem::relief();
                Ok(RenderResponse::Closed)
            }
            RenderRequest::PageSizes { doc } => {
                self.prepare(*doc, None)?;
                if let Some(s) = self.docs.get(doc).and_then(|e| e.sizes.clone()) {
                    return Ok(RenderResponse::PageSizes(s));
                }
                let d = self.doc_mut(*doc)?;
                let mut out = Vec::with_capacity(d.page_count());
                for i in 0..d.page_count() {
                    let (w, h) = d.page_size(i).map_err(map_err)?;
                    out.push(PageSizeInfo {
                        width_pt: w,
                        height_pt: h,
                        rotation: d.page_rotation_uncached(i).unwrap_or(0),
                    });
                }
                if let Some(e) = self.docs.get_mut(doc) {
                    e.sizes = Some(out.clone());
                }
                Ok(RenderResponse::PageSizes(out))
            }
            RenderRequest::RenderTile {
                doc,
                page,
                bucket,
                tile_x,
                tile_y,
                dest,
            } => {
                let p = *page as usize;
                self.prepare(*doc, Some(p))?;
                let mut scratch = std::mem::take(&mut self.scratch);
                let q = self.q.clone();
                let stop = || cancel.is_cancelled() || (tries < 3 && q.higher_than(prio));
                let r = self.doc_mut(*doc).and_then(|d| {
                    d.render_tile_into(
                        p,
                        *bucket,
                        TileCoord {
                            col: *tile_x,
                            row: *tile_y,
                        },
                        &mut scratch,
                        &mut { stop },
                    )
                    .map_err(map_err)
                });
                let out = r.and_then(|(w, h)| self.deliver(w, h, &scratch, *dest));
                self.scratch = scratch;
                if out.is_ok() {
                    bump(&self.stats.tiles);
                }
                out
            }
            RenderRequest::RenderPreview {
                doc,
                page,
                max_edge,
                dest,
            } => {
                if *max_edge == 0 || *max_edge > MAX_PREVIEW_EDGE {
                    return Err(IpcError::invalid(format!(
                        "preview edge must be 1..={MAX_PREVIEW_EDGE}"
                    )));
                }
                let p = *page as usize;
                self.prepare(*doc, Some(p))?;
                let mut scratch = std::mem::take(&mut self.scratch);
                let q = self.q.clone();
                let stop = || cancel.is_cancelled() || (tries < 3 && q.higher_than(prio));
                let r = self.doc_mut(*doc).and_then(|d| {
                    d.render_preview_into(p, *max_edge, &mut scratch, &mut { stop })
                        .map_err(map_err)
                });
                let out = r.and_then(|(w, h)| self.deliver(w, h, &scratch, *dest));
                self.scratch = scratch;
                if out.is_ok() {
                    bump(&self.stats.previews);
                }
                out
            }
            RenderRequest::PageText { doc, page } => {
                bump(&self.stats.text_requests);
                let g = self.glyphs(*doc, *page, true)?;
                Ok(RenderResponse::Text(page_text_info(*page, &g)))
            }
            // Handled by `run`, which owns the search task.
            RenderRequest::Search { .. }
            | RenderRequest::Open { .. }
            | RenderRequest::Reopen { .. }
            | RenderRequest::SetTilePool { .. } => {
                Err(IpcError::internal("request routed to the wrong handler"))
            }
        })();
        // A pause because something more urgent arrived, not a cancel: `run` re-queues it.
        if matches!(&r, Err(e) if e.is_cancelled()) && !cancel.is_cancelled() {
            bump(&self.stats.preempts);
        }
        Outcome::Done(r)
    }

    fn deliver(
        &self,
        w: u32,
        h: u32,
        rgba: &[u8],
        dest: PixelDest,
    ) -> Result<RenderResponse, IpcError> {
        match dest {
            PixelDest::Inline => Ok(RenderResponse::Pixels(PixelsInfo {
                width: w,
                height: h,
                stride: w * 4,
                slot: None,
                inline: Some(rgba.to_vec()),
            })),
            PixelDest::Slot(s) => {
                let pool = self
                    .pool
                    .as_ref()
                    .ok_or_else(|| IpcError::invalid("no tile pool registered"))?;
                if s >= pool.slots {
                    return Err(IpcError::invalid(format!("slot {s} out of range")));
                }
                if rgba.len() > pool.slot_bytes {
                    return Err(IpcError::new(
                        ErrorCode::OutputTooSmall,
                        format!("{} bytes needed", rgba.len()),
                    ));
                }
                pool.region.write_at(s as usize * pool.slot_bytes, rgba);
                Ok(RenderResponse::Pixels(PixelsInfo {
                    width: w,
                    height: h,
                    stride: w * 4,
                    slot: Some(s),
                    inline: None,
                }))
            }
        }
    }

    /// Page text from the cache or PDFium. `store` false keeps bulk readers
    /// (search) from evicting the visible pages' text.
    fn glyphs(&mut self, doc: DocId, page: u32, store: bool) -> Result<Arc<Glyphs>, IpcError> {
        if store {
            if let Some(g) = self.text.get((doc, page)) {
                bump(&self.stats.text_cache_hits);
                return Ok(g);
            }
        } else if let Some(g) = self.text.peek((doc, page)) {
            return Ok(g);
        }
        self.prepare(doc, Some(page as usize))?;
        let g = Arc::new(
            self.doc_mut(doc)?
                .page_glyphs(page as usize)
                .map_err(map_err)?,
        );
        if store {
            self.text.put((doc, page), g.clone());
        }
        Ok(g)
    }

    // -------------------------------------------------------------- search

    fn start_search(
        &mut self,
        doc: DocId,
        q: &SearchQuery,
        job: bool,
    ) -> Result<SearchTask, IpcError> {
        self.prepare(doc, None)?;
        let opts = SearchOptions {
            whole_word: q.whole_word,
            case_sensitive: q.case_sensitive,
            ..SearchOptions::default()
        };
        let searcher =
            Searcher::new(&q.text, &opts).ok_or_else(|| IpcError::invalid("empty search text"))?;
        Ok(SearchTask {
            doc,
            searcher,
            next: 0,
            hits: 0,
            max_hits: if q.max_hits == 0 {
                usize::MAX
            } else {
                q.max_hits as usize
            },
            streaming: job,
            batch: Vec::new(),
            all: Vec::new(),
            first_sent: false,
            last_flush: Instant::now(),
            truncated: false,
        })
    }

    fn flush_partial(&self, job: Option<JobId>, t: &mut SearchTask) {
        if t.batch.is_empty() {
            return;
        }
        t.last_flush = Instant::now();
        let hits = std::mem::take(&mut t.batch);
        if let (true, Some(job)) = (t.streaming, job) {
            let _ = self.tx.send(&ChildToHost::Partial {
                job,
                value: RenderResponse::Hits {
                    hits,
                    complete: false,
                },
            });
        } else {
            t.all.extend(hits);
        }
    }

    /// Run a slice of the search. `Ok(None)` means "not finished, requeue".
    fn search_slice(
        &mut self,
        job: Option<JobId>,
        cancel: &CancelToken,
        t: &mut SearchTask,
    ) -> Result<Option<RenderResponse>, IpcError> {
        let start = Instant::now();
        let page_count = self
            .docs
            .get(&t.doc)
            .map(|e| e.page_count as usize)
            .ok_or_else(|| unknown_doc(t.doc))?;
        while t.next < page_count {
            if cancel.is_cancelled() {
                return Err(IpcError::cancelled());
            }
            if start.elapsed() >= self.cfg.search_slice || self.q.higher_than(Priority::Search) {
                self.flush_partial(job, t);
                return Ok(None);
            }
            let p = t.next as u32;
            t.next += 1;
            bump(&self.stats.search_pages);
            let was_cached = self
                .docs
                .get(&t.doc)
                .and_then(|e| e.doc.as_ref())
                .is_some_and(|d| d.is_page_cached(p as usize));
            let g = match self.glyphs(t.doc, p, false) {
                Ok(g) => g,
                Err(e) if e.code == ErrorCode::NotFound => return Err(e),
                Err(_) => continue, // an unreadable page has no text
            };
            if !was_cached && let Some(d) = self.docs.get_mut(&t.doc).and_then(|e| e.doc.as_mut()) {
                d.close_page(p as usize);
            }
            let input = PageInput {
                page: p as usize,
                text: PageChars::new(
                    g.chars.clone(),
                    g.loose
                        .iter()
                        .map(|b| Rect::new(b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64))
                        .collect(),
                ),
                extras: Vec::new(),
            };
            let (max_hits, mut hits, mut truncated) = (t.max_hits, t.hits, false);
            let mut found = Vec::new();
            t.searcher.search_page(&input, &mut |h| {
                if hits >= max_hits {
                    truncated = true;
                    return false;
                }
                hits += 1;
                let s = &h.snippet;
                found.push(WireHit {
                    page: p,
                    quads: h.quads.iter().map(|q| Quad(q.to_array())).collect(),
                    context: format!("{}{}{}", s.before, s.matched, s.after),
                });
                true
            });
            t.hits = hits;
            t.truncated |= truncated;
            // First-hit fast path: the first hit goes out alone, at once.
            if !t.first_sent && !found.is_empty() {
                t.first_sent = true;
                t.batch.push(found.remove(0));
                self.flush_partial(job, t);
            }
            t.batch.extend(found);
            if t.batch.len() >= 32 || t.last_flush.elapsed() >= Duration::from_millis(50) {
                self.flush_partial(job, t);
            }
            if t.truncated {
                break;
            }
        }
        self.flush_partial(job, t);
        let hits = if t.streaming && job.is_some() {
            Vec::new()
        } else {
            std::mem::take(&mut t.all)
        };
        Ok(Some(RenderResponse::Hits {
            hits,
            complete: !t.truncated,
        }))
    }

    // ----------------------------------------------------------- dispatch

    fn run(&mut self, item: Item<Work>) {
        let Item {
            id,
            job,
            cancel,
            prio,
            work,
            ..
        } = item;
        let res = catch_unwind(AssertUnwindSafe(|| match work {
            Work::Search(mut t) => {
                if cancel.is_cancelled() {
                    return Outcome::Done(Err(IpcError::cancelled()));
                }
                match self.search_slice(job, &cancel, &mut t) {
                    Ok(None) => Outcome::Requeue(Work::Search(t)),
                    Ok(Some(r)) => Outcome::Done(Ok(r)),
                    Err(e) => Outcome::Done(Err(e)),
                }
            }
            Work::Req { req, tries } => {
                if cancel.is_cancelled() {
                    return Outcome::Done(Err(IpcError::cancelled()));
                }
                match req {
                    RenderRequest::SetTilePool {
                        pool,
                        slot_bytes,
                        slots,
                    } => {
                        let sb = usize::try_from(slot_bytes).unwrap_or(0);
                        if sb == 0 || (sb as u128 * slots as u128) > pool.len() as u128 {
                            return Outcome::Done(Err(IpcError::invalid(
                                "tile pool smaller than slots * slot_bytes",
                            )));
                        }
                        self.pool = Some(Pool {
                            region: pool,
                            slot_bytes: sb,
                            slots,
                        });
                        Outcome::Done(Ok(RenderResponse::PoolReady))
                    }
                    RenderRequest::Open {
                        doc,
                        base,
                        sections,
                        password,
                    } => {
                        let r: Result<RenderResponse, IpcError> = (|| {
                            let base = base.into_bytes()?;
                            let secs = sources(sections)?;
                            let e = self.open_entry(base, secs, password)?;
                            let n = e.page_count;
                            self.text.invalidate_doc(doc);
                            self.docs.insert(doc, e);
                            Ok(RenderResponse::Opened { doc, page_count: n })
                        })();
                        Outcome::Done(r)
                    }
                    RenderRequest::Reopen { doc, sections } => {
                        bump(&self.stats.reopens_requested);
                        let r = (|| {
                            let secs = sources(sections)?;
                            let debounce = self.cfg.debounce;
                            let e = self.docs.get_mut(&doc).ok_or_else(|| unknown_doc(doc))?;
                            let mut replies =
                                e.pending.take().map(|p| p.replies).unwrap_or_default();
                            replies.push(id);
                            e.pending = Some(PendingReopen {
                                sections: secs,
                                due: Instant::now() + debounce,
                                replies,
                            });
                            Ok(())
                        })();
                        match r {
                            Ok(()) => Outcome::Deferred,
                            Err(e) => Outcome::Done(Err(e)),
                        }
                    }
                    RenderRequest::Search { doc, query } => {
                        match self.start_search(doc, &query, job.is_some()) {
                            Ok(t) => Outcome::Requeue(Work::Search(Box::new(t))),
                            Err(e) => Outcome::Done(Err(e)),
                        }
                    }
                    other => match self.handle(prio, &cancel, &other, tries) {
                        Outcome::Done(Err(e)) if e.is_cancelled() && !cancel.is_cancelled() => {
                            Outcome::Requeue(Work::Req {
                                req: other,
                                tries: tries + 1,
                            })
                        }
                        o => o,
                    },
                }
            }
        }));
        let outcome = res.unwrap_or_else(|p| {
            Outcome::Done(Err(IpcError::new(ErrorCode::Internal, panic_message(&*p))))
        });
        match outcome {
            Outcome::Deferred => {}
            Outcome::Requeue(w) => {
                let prio = match &w {
                    Work::Search(_) => Priority::Search,
                    Work::Req { .. } => prio,
                };
                self.q.push(prio, id, job, w);
            }
            Outcome::Done(r) => {
                if let Some(j) = job {
                    self.q.finish_job(j);
                }
                self.send(id, r);
            }
        }
        self.after_job();
    }

    // ------------------------------------------------------------ housekeeping

    fn next_deadline(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut best: Option<Duration> = None;
        let mut consider = |d: Duration| best = Some(best.map_or(d, |b| b.min(d)));
        for e in self.docs.values() {
            if let Some(p) = &e.pending {
                consider(p.due.saturating_duration_since(now));
            }
        }
        if !self.docs.is_empty() {
            let idle = now.saturating_duration_since(self.last_activity);
            let m = &self.cfg.memory;
            if self.idle_stage == 0 {
                consider(m.idle_after.saturating_sub(idle));
            } else if self.idle_stage == 1 {
                consider(m.hibernate_after.saturating_sub(idle));
            }
        }
        best
    }

    fn housekeeping(&mut self) {
        let now = Instant::now();
        let due: Vec<DocId> = self
            .docs
            .iter()
            .filter(|(_, e)| e.pending.as_ref().is_some_and(|p| p.due <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.apply_pending(id);
        }
        if self.docs.is_empty() || !self.q.is_empty() {
            return;
        }
        let idle = now.saturating_duration_since(self.last_activity);
        let (idle_after, hib_after) = (self.cfg.memory.idle_after, self.cfg.memory.hibernate_after);
        if self.idle_stage == 0 && idle >= idle_after {
            self.idle_stage = 1;
            self.trim_pages(None);
            let growth = self.growth();
            if self.gov.idle_should_recycle(growth) {
                let ids: Vec<DocId> = self.docs.keys().copied().collect();
                for id in ids {
                    let loaded = self
                        .docs
                        .get(&id)
                        .and_then(|e| e.doc.as_ref())
                        .is_some_and(|d| d.pages_loaded() > 0);
                    if loaded {
                        let _ = self.recycle(id, growth);
                    }
                }
            }
            mem::relief();
        }
        if self.idle_stage == 1 && idle >= hib_after {
            self.idle_stage = 2;
            let mut any = false;
            for e in self.docs.values_mut() {
                if e.doc.is_some() && e.pending.is_none() && self.gov.should_hibernate(e.open_cost)
                {
                    e.doc = None;
                    any = true;
                    bump(&self.stats.hibernations);
                }
            }
            if any {
                mem::relief();
            }
        }
    }

    fn warmup(&mut self) {
        let Ok(lib) = self.lib() else { return };
        let t = Instant::now();
        let b = testgen::build(&[testgen::PageSpec::letter(testgen::hello_content())]);
        if let Ok(mut d) = Document::open(&lib, crate::bytes_from_vec(b.bytes), &[], None) {
            let _ = d.render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut crate::never_cancel);
            let _ = d.page_glyphs(0);
        }
        self.stats
            .warmup_us
            .store(t.elapsed().as_micros() as u64, Ordering::Relaxed);
        mem::relief();
    }
}

fn page_text_info(page: u32, g: &Glyphs) -> PageTextInfo {
    PageTextInfo {
        page,
        text: g.chars.iter().collect(),
        chars: g
            .chars
            .iter()
            .zip(&g.tight)
            .map(|(&ch, b)| papyrine_ipc::CharBoxInfo {
                ch,
                left: b[0] as f64,
                right: b[2] as f64,
                bottom: b[1] as f64,
                top: b[3] as f64,
            })
            .collect(),
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("panic: {s}")
    } else {
        "panic".into()
    }
}

fn reader_loop(
    mut rx: Receiver,
    q: Arc<Queue<Work>>,
    tx: Arc<Sender>,
    exit: Arc<Mutex<ServeExit>>,
) {
    loop {
        match rx.recv::<HostToChild<RenderRequest>>() {
            Ok(Some(HostToChild::Request { id, job, body })) => {
                let prio = classify(&body, job);
                q.push(
                    prio,
                    id,
                    job,
                    Work::Req {
                        req: body,
                        tries: 0,
                    },
                );
            }
            Ok(Some(HostToChild::Cancel { job })) => {
                if let Some(it) = q.cancel(job) {
                    let _ = tx.send(&ChildToHost::<RenderResponse>::Response {
                        id: it.id,
                        result: Err(IpcError::cancelled()),
                    });
                }
            }
            Ok(Some(HostToChild::Hello(_))) => {}
            Ok(Some(HostToChild::Shutdown)) => {
                *exit.lock().unwrap_or_else(|p| p.into_inner()) = ServeExit::Shutdown;
                q.close();
                return;
            }
            Ok(None) | Err(_) => {
                q.close();
                return;
            }
        }
    }
}

/// Run the renderer role on the calling thread until shutdown or host
/// disconnect. The calling thread becomes the PDFium thread.
pub fn serve(
    endpoint: Endpoint,
    config: RendererConfig,
    stats: Arc<Stats>,
) -> Result<ServeExit, ServeError> {
    let Endpoint { tx, mut rx } = endpoint;
    let tx = Arc::new(tx);

    let hello = match rx.recv::<HostToChild<RenderRequest>>()? {
        Some(HostToChild::Hello(h)) => h,
        _ => return Err(ServeError::NoHello),
    };
    tx.send(&ChildToHost::<RenderResponse>::Hello(Hello::new(
        Role::Renderer,
    )))?;
    if negotiate((PROTOCOL_MIN, PROTOCOL_VERSION), (hello.min, hello.max)).is_none() {
        return Err(ServeError::VersionMismatch(hello.min, hello.max));
    }

    let q: Arc<Queue<Work>> = Arc::new(Queue::new());
    let exit = Arc::new(Mutex::new(ServeExit::HostGone));
    {
        let (q, tx, exit) = (q.clone(), tx.clone(), exit.clone());
        std::thread::Builder::new()
            .name("papyrine-render-ipc".into())
            .spawn(move || reader_loop(rx, q, tx, exit))
            .map_err(TransportError::Io)?;
    }

    let lib = Library::load(config.pdfium_dir.as_deref()).map_err(|e| e.to_string());
    let mut s = Server {
        lib,
        gov: Governor::new(config.memory.clone()),
        text: TextCache::new(config.memory.text_cache_bytes),
        cfg: config,
        docs: HashMap::new(),
        pool: None,
        scratch: Vec::new(),
        tx,
        q: q.clone(),
        stats,
        floor: 0,
        active: None,
        last_activity: Instant::now(),
        idle_stage: 0,
    };
    if s.cfg.warmup {
        s.warmup();
    }
    s.floor = mem::footprint();
    s.stats.floor_bytes.store(s.floor, Ordering::Relaxed);
    s.last_activity = Instant::now();

    loop {
        match q.pop(s.next_deadline()) {
            Popped::Item(it) => s.run(it),
            Popped::Timeout => s.housekeeping(),
            Popped::Closed => break,
        }
    }
    drop(s);
    let e = *exit.lock().unwrap_or_else(|p| p.into_inner());
    Ok(e)
}

/// [`serve`] with default configuration; what the multi-role executable calls.
pub fn run_renderer(endpoint: Endpoint) -> Result<ServeExit, ServeError> {
    serve(endpoint, RendererConfig::default(), Arc::default())
}
