//! Renderer side of the broker: the shared tile pool, document (re)opening, tiles with
//! L1/L2 caching, text, search jobs and snapshot rebasing.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use papyrine_ipc::{
    Client, DocId, DocSource, ErrorCode, Handle, IpcError, PageTextInfo, PixelDest, PixelsInfo,
    RenderRequest, RenderResponse, SearchQuery, SharedRegion,
};

use super::engine::RenderAction;
use super::{Broker, JobCtl, POOL_SLOTS, SLOT_BYTES};
use crate::api::{HostEvent, PageInfo, PageText, SearchHit, SearchOptions, TextRun};
use crate::cache::{PreviewKey, TileData, TileKey};
use crate::error::{Code, HostErr, Result, is_crash};
use crate::sched::{Callback, PRIO_PREVIEW, PRIO_VISIBLE};
use crate::session::Session;

pub(crate) type RenderClient = Client<RenderRequest, RenderResponse>;

/// Previews use `bucket = PREVIEW_BASE - edge` in the scheduler's key space.
pub(crate) const PREVIEW_BASE: i32 = -1000;
pub const MIN_BUCKET: i32 = -8;
pub const MAX_BUCKET: i32 = 16;

/// Zoom bucket (`scale = sqrt(2)^bucket`) nearest to a device-px-per-point scale.
pub fn bucket_for_scale(scale: f64) -> i32 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0;
    }
    ((scale.log2() * 2.0).round() as i32).clamp(MIN_BUCKET, MAX_BUCKET)
}

fn pages_from(sizes: Vec<papyrine_ipc::PageSizeInfo>) -> Vec<PageInfo> {
    sizes
        .into_iter()
        .map(|p| PageInfo {
            width: p.width_pt,
            height: p.height_pt,
            rotation: p.rotation,
            label: None,
        })
        .collect()
}

/// A slot of the shared tile pool, returned on drop.
struct Slot<'a> {
    broker: &'a Broker,
    idx: u32,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut p = self
            .broker
            .inner
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        p.free.push(self.idx);
        drop(p);
        self.broker.inner.pool_cv.notify_one();
    }
}

impl Broker {
    // ---------------------------------------------------------------- connection

    fn ensure_pool(&self, client: &RenderClient, generation: u64) -> Result<()> {
        let mut p = self
            .inner
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if p.generation == generation {
            return Ok(());
        }
        let region = match p.region.clone() {
            Some(r) => r,
            None => Arc::new(SharedRegion::create(
                (SLOT_BYTES * POOL_SLOTS as u64) as usize,
            )?),
        };
        client.call_blocking(RenderRequest::SetTilePool {
            pool: crate::util::dup_region(&region)?,
            slot_bytes: SLOT_BYTES,
            slots: POOL_SLOTS,
        })?;
        p.region = Some(region);
        p.generation = generation;
        Ok(())
    }

    /// The renderer with the pool registered and this document open at the current
    /// snapshot. Restarts the renderer if it died (it reloads from the snapshot).
    pub(crate) fn render_client(&self, s: &Session) -> Result<RenderClient> {
        let _g = s.render_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let c = self.inner.renderer.conn()?;
        self.ensure_pool(&c.client, c.generation)?;
        if s.st().render_gen != c.generation {
            self.render_open(s, &c.client)?;
            s.st().render_gen = c.generation;
        }
        Ok(c.client)
    }

    /// (Re)open the document in the renderer from the session's base and sections. A stale
    /// copy (older base, or left from before a restart) is closed first.
    fn render_open(&self, s: &Session, client: &RenderClient) -> Result<()> {
        let (base, sections, password) = {
            let st = s.st();
            let sections: Result<Vec<DocSource>> = st.sections.iter().map(|x| x.source()).collect();
            (
                st.render_source_base().source()?,
                sections?,
                st.password.clone(),
            )
        };
        let _ = client.call_blocking(RenderRequest::Close { doc: DocId(s.id) });
        client.call_blocking(RenderRequest::Open {
            doc: DocId(s.id),
            base,
            sections,
            password,
        })?;
        Ok(())
    }

    pub(crate) fn with_render<T>(
        &self,
        s: &Session,
        f: impl Fn(&RenderClient) -> std::result::Result<T, IpcError>,
    ) -> Result<T> {
        for attempt in 0..2 {
            let client = self.render_client(s)?;
            match f(&client) {
                Err(e) if is_crash(&e) && attempt == 0 => continue,
                r => return r.map_err(Into::into),
            }
        }
        Err(HostErr::internal("renderer kept crashing"))
    }

    fn fetch_page_sizes(&self, s: &Session) -> Result<Vec<PageInfo>> {
        let doc = DocId(s.id);
        match self.with_render(s, |c| c.call_blocking(RenderRequest::PageSizes { doc }))? {
            RenderResponse::PageSizes(v) => Ok(pages_from(v)),
            other => Err(HostErr::internal(format!("renderer: unexpected {other:?}"))),
        }
    }

    /// First open: open in the renderer and read every page size.
    pub(crate) fn render_open_info(&self, s: &Session) -> Result<Vec<PageInfo>> {
        self.fetch_page_sizes(s)
    }

    /// After a commit: point the renderer at the new section list.
    pub(crate) fn render_reopen_pages(&self, s: &Session) -> Result<Vec<PageInfo>> {
        let doc = DocId(s.id);
        self.with_render(s, |c| {
            // A renderer that was (re)started opened with the full section list already;
            // a Reopen with the same list is then a cheap no-op.
            let secs = s
                .st()
                .sections
                .iter()
                .map(|x| x.source().map_err(|e| IpcError::internal(e.message)))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            c.call_blocking(RenderRequest::Reopen {
                doc,
                sections: secs,
            })
        })?;
        self.fetch_page_sizes(s)
    }

    // ------------------------------------------------------------------ invalidation

    pub(crate) fn invalidate_doc(&self, doc: u64) {
        self.inner
            .l1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate_doc(doc);
        self.inner
            .l2
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate_doc(doc);
    }

    pub(crate) fn invalidate_pages(&self, doc: u64, pages: &[u32]) {
        self.inner
            .l1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate_pages(doc, pages);
        self.inner
            .l2
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate_pages(doc, pages);
    }

    // --------------------------------------------------------------------- tiles

    fn acquire_slot(&self) -> Result<Slot<'_>> {
        let mut p = self
            .inner
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(idx) = p.free.pop() {
                return Ok(Slot { broker: self, idx });
            }
            let (g, to) = self
                .inner
                .pool_cv
                .wait_timeout(p, Duration::from_secs(30))
                .unwrap_or_else(PoisonError::into_inner);
            p = g;
            if to.timed_out() && p.free.is_empty() {
                return Err(HostErr::internal("no free tile slot"));
            }
        }
    }

    fn read_pixels(&self, info: PixelsInfo) -> Result<TileData> {
        let (w, h) = (info.width as usize, info.height as usize);
        if w == 0 || h == 0 || w > 1 << 16 || h > 1 << 16 {
            return Err(HostErr::internal("renderer returned an empty tile"));
        }
        let row = w * 4;
        let stride = info.stride as usize;
        if stride < row {
            return Err(HostErr::internal("renderer returned a bad stride"));
        }
        if let Some(px) = info.inline {
            if stride == row && px.len() == row * h {
                return Ok(TileData {
                    width: info.width,
                    height: info.height,
                    rgba: px,
                });
            }
            let mut rgba = Vec::with_capacity(row * h);
            for y in 0..h {
                rgba.extend_from_slice(
                    px.get(y * stride..y * stride + row)
                        .ok_or_else(|| HostErr::internal("short inline tile"))?,
                );
            }
            return Ok(TileData {
                width: info.width,
                height: info.height,
                rgba,
            });
        }
        let slot = info
            .slot
            .ok_or_else(|| HostErr::internal("tile has no pixels"))?;
        let region = self
            .inner
            .pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .region
            .clone()
            .ok_or_else(|| HostErr::internal("no tile pool"))?;
        let base = slot as usize * SLOT_BYTES as usize;
        if slot >= POOL_SLOTS || stride * h > SLOT_BYTES as usize {
            return Err(HostErr::internal("tile outside its slot"));
        }
        let mut rgba = vec![0u8; row * h];
        if stride == row {
            region.read_at(base, &mut rgba);
        } else {
            for y in 0..h {
                region.read_at(base + y * stride, &mut rgba[y * row..(y + 1) * row]);
            }
        }
        Ok(TileData {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    /// Scheduler worker entry: render one tile (or preview) now.
    pub(crate) fn render_tile_now(
        &self,
        key: &TileKey,
        cancel: &AtomicBool,
    ) -> Result<Arc<TileData>> {
        let s = self.session_by_id(key.doc).ok_or_else(HostErr::cancelled)?;
        let preview_edge = (key.bucket <= PREVIEW_BASE).then(|| (PREVIEW_BASE - key.bucket) as u32);
        for attempt in 0..3 {
            if cancel.load(Ordering::Relaxed) {
                return Err(HostErr::cancelled());
            }
            let epoch = s.st().epoch;
            let slot = self.acquire_slot()?;
            let client = self.render_client(&s)?;
            let doc = DocId(s.id);
            let dest = PixelDest::Slot(slot.idx);
            let req = match preview_edge {
                Some(edge) => RenderRequest::RenderPreview {
                    doc,
                    page: key.page,
                    max_edge: edge,
                    dest: if edge <= 512 { dest } else { PixelDest::Inline },
                },
                None => RenderRequest::RenderTile {
                    doc,
                    page: key.page,
                    bucket: key.bucket,
                    tile_x: key.tx,
                    tile_y: key.ty,
                    dest,
                },
            };
            let job = client.start_job(req)?;
            let mut cancel_sent = false;
            let result = loop {
                if let Some(r) = job.wait_timeout(Duration::from_millis(4)) {
                    break r;
                }
                if !cancel_sent && cancel.load(Ordering::Relaxed) {
                    job.cancel();
                    cancel_sent = true;
                }
            };
            // The reply is consumed; this only unregisters the job from the client.
            let _ = job.wait();
            let pixels = match result {
                Ok(RenderResponse::Pixels(p)) => p,
                Ok(other) => {
                    return Err(HostErr::internal(format!("renderer: unexpected {other:?}")));
                }
                Err(e) if is_crash(&e) && attempt < 2 => continue,
                Err(e) => return Err(e.into()),
            };
            let tile = Arc::new(self.read_pixels(pixels)?);
            drop(slot);
            if s.st().epoch != epoch {
                // The page changed while rendering; do not cache the stale pixels.
                if attempt < 2 {
                    continue;
                }
                return Ok(tile);
            }
            match preview_edge {
                Some(edge) => self
                    .inner
                    .l2
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .put(
                        PreviewKey {
                            doc: key.doc,
                            page: key.page,
                            edge,
                        },
                        &tile,
                    ),
                None => self
                    .inner
                    .l1
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .put(*key, tile.clone()),
            }
            return Ok(tile);
        }
        Err(HostErr::internal("tile render kept failing"))
    }

    /// Ask for a tile; `cb` runs on a scheduler thread (or immediately on a cache hit).
    #[allow(clippy::too_many_arguments)]
    pub fn request_tile(
        &self,
        doc_id: &str,
        page: u32,
        scale: f64,
        tx: u32,
        ty: u32,
        prio: u8,
        rid: u64,
        cb: Callback,
    ) {
        let s = match self.session(doc_id) {
            Ok(s) => s,
            Err(e) => return cb(Err(e)),
        };
        if page as usize >= s.st().pages.len() {
            return cb(Err(HostErr::new(Code::NotFound, format!("no page {page}"))));
        }
        let key = TileKey {
            doc: s.id,
            page,
            bucket: bucket_for_scale(scale),
            tx,
            ty,
        };
        {
            let mut l1 = self.inner.l1.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(t) = l1.get(&key) {
                drop(l1);
                return cb(Ok(t));
            }
            l1.note_request();
        }
        self.inner.sched().submit(key, prio, rid, cb);
    }

    /// Blocking convenience for tests and the bridge.
    pub fn get_tile_blocking(
        &self,
        doc_id: &str,
        page: u32,
        scale: f64,
        tx: u32,
        ty: u32,
    ) -> Result<Arc<TileData>> {
        let (tx_, rx) = std::sync::mpsc::channel();
        self.request_tile(
            doc_id,
            page,
            scale,
            tx,
            ty,
            PRIO_VISIBLE,
            u64::MAX,
            Box::new(move |r| {
                let _ = tx_.send(r);
            }),
        );
        rx.recv()
            .map_err(|_| HostErr::internal("tile scheduler stopped"))?
    }

    pub fn cancel_tile(&self, rid: u64) {
        self.inner.sched().cancel(rid);
    }

    /// Low-resolution page preview (L2, QOI-compressed). `edge` is the longest side.
    pub fn request_preview(&self, doc_id: &str, page: u32, edge: u32, rid: u64, cb: Callback) {
        let s = match self.session(doc_id) {
            Ok(s) => s,
            Err(e) => return cb(Err(e)),
        };
        let edge = edge.clamp(16, 2048);
        if page as usize >= s.st().pages.len() {
            return cb(Err(HostErr::new(Code::NotFound, format!("no page {page}"))));
        }
        let pk = PreviewKey {
            doc: s.id,
            page,
            edge,
        };
        if let Some(t) = self
            .inner
            .l2
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pk)
        {
            return cb(Ok(Arc::new(t)));
        }
        let key = TileKey {
            doc: s.id,
            page,
            bucket: PREVIEW_BASE - edge as i32,
            tx: 0,
            ty: 0,
        };
        self.inner.sched().submit(key, PRIO_PREVIEW, rid, cb);
    }

    // ---------------------------------------------------------------------- text

    pub fn get_page_text(&self, doc_id: &str, page: usize) -> Result<PageText> {
        let s = self.session(doc_id)?;
        let (info, pw, ph, rot) = {
            let st = s.st();
            let p = st
                .pages
                .get(page)
                .ok_or_else(|| HostErr::new(Code::NotFound, format!("no page {page}")))?;
            (p.clone(), p.width, p.height, p.rotation)
        };
        let _ = info;
        let doc = DocId(s.id);
        let r = self.with_render(&s, |c| {
            c.call_blocking(RenderRequest::PageText {
                doc,
                page: page as u32,
            })
        })?;
        let RenderResponse::Text(t) = r else {
            return Err(HostErr::internal(format!("renderer: unexpected {r:?}")));
        };
        Ok(PageText {
            page,
            runs: runs_from(&t, pw, ph, rot),
        })
    }

    // -------------------------------------------------------------------- search

    /// Start a search job; hits stream as `search-hit` events.
    pub fn search(&self, doc_id: &str, query: &str, opts: SearchOptions) -> Result<String> {
        let s = self.session(doc_id)?;
        let id = self.inner.next_job.fetch_add(1, Ordering::Relaxed);
        let job_id = format!("job-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                JobCtl {
                    cancel: cancel.clone(),
                },
            );
        let b = self.clone();
        let query = query.to_string();
        let jid = job_id.clone();
        std::thread::Builder::new()
            .name("papyrine-search".into())
            .spawn(move || {
                b.run_search(&s, &jid, &query, opts, &cancel);
                b.inner
                    .jobs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
            })
            .map_err(|e| HostErr::internal(e.to_string()))?;
        Ok(job_id)
    }

    pub fn cancel_job(&self, job_id: &str) {
        let Some(id) = job_id
            .strip_prefix("job-")
            .and_then(|n| n.parse::<u64>().ok())
        else {
            return;
        };
        if let Some(j) = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
        {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn run_search(
        &self,
        s: &Arc<Session>,
        job_id: &str,
        query: &str,
        opts: SearchOptions,
        cancel: &AtomicBool,
    ) {
        let total = s.st().pages.len() as u64;
        let finish = |done: u64| {
            self.emit(HostEvent::JobProgress {
                job_id: job_id.into(),
                done,
                total,
                finished: true,
            });
        };
        let client = match self.render_client(s) {
            Ok(c) => c,
            Err(e) => {
                self.notice(
                    "warn",
                    "search-failed",
                    &format!("Search failed: {}", e.message),
                    Some(s),
                );
                return finish(0);
            }
        };
        let job = match client.start_job(RenderRequest::Search {
            doc: DocId(s.id),
            query: SearchQuery {
                text: query.to_string(),
                case_sensitive: opts.case_sensitive,
                whole_word: opts.whole_word,
                max_hits: 10_000,
            },
        }) {
            Ok(j) => j,
            Err(e) => {
                self.notice(
                    "warn",
                    "search-failed",
                    &format!("Search failed: {}", e.message),
                    Some(s),
                );
                return finish(0);
            }
        };
        let mut seen: HashSet<(u32, u64, String)> = HashSet::new();
        let mut done = 0u64;
        let mut cancel_sent = false;
        let emit_hits = |hits: Vec<papyrine_ipc::SearchHit>,
                         seen: &mut HashSet<(u32, u64, String)>| {
            for h in hits {
                let sig = h
                    .quads
                    .first()
                    .map_or(0, |q| q.0[0].to_bits() ^ q.0[1].to_bits().rotate_left(17));
                if !seen.insert((h.page, sig, h.context.clone())) {
                    continue;
                }
                self.emit(HostEvent::SearchHit {
                    job_id: job_id.into(),
                    hit: map_hit(&h, query, opts.case_sensitive),
                });
            }
        };
        let result = loop {
            while let Ok(p) = job.progress.try_recv() {
                done = p.done;
                self.emit(HostEvent::JobProgress {
                    job_id: job_id.into(),
                    done: p.done,
                    total: p.total.max(total),
                    finished: false,
                });
            }
            while let Ok(RenderResponse::Hits { hits, .. }) = job.partials.try_recv() {
                emit_hits(hits, &mut seen);
            }
            if !cancel_sent && cancel.load(Ordering::Relaxed) {
                job.cancel();
                cancel_sent = true;
            }
            if let Some(r) = job.wait_timeout(Duration::from_millis(15)) {
                break r;
            }
        };
        // Partials sent just before the final response may still be queued.
        while let Ok(RenderResponse::Hits { hits, .. }) = job.partials.try_recv() {
            emit_hits(hits, &mut seen);
        }
        let _ = job.wait();
        match result {
            Ok(RenderResponse::Hits { hits, .. }) => {
                emit_hits(hits, &mut seen);
                done = total;
            }
            Err(e) if !e.is_cancelled() => {
                self.notice(
                    "warn",
                    "search-failed",
                    &format!("Search failed: {}", e.message),
                    Some(s),
                );
            }
            _ => {}
        }
        finish(done);
    }

    // ------------------------------------------------------------------ snapshot

    pub(crate) fn render_base_path(&self, s: &Session) -> PathBuf {
        self.inner
            .cfg
            .cache_dir
            .join("render")
            .join(format!("doc-{}.pdf", s.id))
    }

    /// Bring the renderer in line with what an engine snapshot update changed, and
    /// refresh page sizes (a rotate or delete changes them).
    pub(crate) fn render_sync(&self, s: &Session, action: RenderAction) -> Result<()> {
        let pages = match action {
            RenderAction::None => return Ok(()),
            RenderAction::Reopen => self.render_reopen_pages(s)?,
            RenderAction::Rebase => {
                {
                    let _g = s.render_lock.lock().unwrap_or_else(PoisonError::into_inner);
                    let c = self.inner.renderer.conn()?;
                    self.ensure_pool(&c.client, c.generation)?;
                    self.render_open(s, &c.client)?;
                    s.st().render_gen = c.generation;
                }
                self.fetch_page_sizes(s)?
            }
        };
        let mut st = s.st();
        st.pages = pages;
        st.apply_labels();
        Ok(())
    }

    // ------------------------------------------------------------ save validation

    /// Open `path` in the renderer and render a small view of page 1 (save validation).
    pub(crate) fn render_check(
        &self,
        path: &Path,
        password: Option<String>,
    ) -> std::result::Result<(), String> {
        let c = self.inner.renderer.conn().map_err(|e| e.message)?;
        self.ensure_pool(&c.client, c.generation)
            .map_err(|e| e.message)?;
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let doc = DocId(u64::MAX - self.inner.temp_ids.fetch_add(1, Ordering::Relaxed));
        c.client
            .call_blocking(RenderRequest::Open {
                doc,
                base: DocSource::File {
                    handle: Handle::from_file(f),
                    len,
                },
                sections: vec![],
                password,
            })
            .map_err(|e| e.message)?;
        let r = c.client.call_blocking(RenderRequest::RenderPreview {
            doc,
            page: 0,
            max_edge: 96,
            dest: PixelDest::Inline,
        });
        let _ = c.client.call_blocking(RenderRequest::Close { doc });
        match r {
            Ok(RenderResponse::Pixels(p)) if p.width > 0 && p.height > 0 => Ok(()),
            Ok(other) => Err(format!("unexpected renderer reply {other:?}")),
            Err(e) if e.code == ErrorCode::Cancelled => Err("cancelled".into()),
            Err(e) => Err(e.message),
        }
    }
}

// ------------------------------------------------------------------------ helpers

fn map_hit(h: &papyrine_ipc::SearchHit, query: &str, case_sensitive: bool) -> SearchHit {
    let (hay, needle) = if case_sensitive {
        (h.context.clone(), query.to_string())
    } else {
        (h.context.to_lowercase(), query.to_lowercase())
    };
    let utf16 = |s: &str| s.encode_utf16().count();
    // Lowercasing can change byte lengths for exotic scripts; fall back to the start.
    let (start, len) = match hay.find(&needle) {
        Some(i) if hay.len() == h.context.len() => (utf16(&h.context[..i]), utf16(query)),
        _ => (0, utf16(query)),
    };
    SearchHit {
        page: h.page as usize,
        snippet: h.context.clone(),
        match_start: start,
        match_length: len,
    }
}

/// Convert PDFium user-space character boxes into positioned text runs in displayed-page
/// space (points, origin top-left).
pub(crate) fn runs_from(t: &PageTextInfo, disp_w: f32, disp_h: f32, rotation: u16) -> Vec<TextRun> {
    let (dw, dh) = (disp_w as f64, disp_h as f64);
    // Unrotated page size in user space.
    let (w0, h0) = if rotation % 180 == 90 {
        (dh, dw)
    } else {
        (dw, dh)
    };
    let map = |x: f64, y: f64| -> (f64, f64) {
        match rotation % 360 {
            90 => (y, x),
            180 => (w0 - x, y),
            270 => (h0 - y, w0 - x),
            _ => (x, h0 - y),
        }
    };
    struct Run {
        text: String,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
    }
    let mut out: Vec<TextRun> = Vec::new();
    let mut cur: Option<Run> = None;
    let flush = |cur: &mut Option<Run>, out: &mut Vec<TextRun>| {
        if let Some(r) = cur.take() {
            let text = r.text.trim_end().to_string();
            if !text.trim().is_empty() && r.x1 > r.x0 && r.y1 > r.y0 {
                out.push(TextRun {
                    text,
                    x: r.x0,
                    y: r.y0,
                    width: r.x1 - r.x0,
                    height: r.y1 - r.y0,
                });
            }
        }
    };
    for c in &t.chars {
        if matches!(c.ch, '\r' | '\n' | '\u{2}' | '\u{0}') {
            flush(&mut cur, &mut out);
            continue;
        }
        let (ax, ay) = map(c.left, c.bottom);
        let (bx, by) = map(c.right, c.top);
        let (x0, x1) = (ax.min(bx), ax.max(bx));
        let (y0, y1) = (ay.min(by), ay.max(by));
        let empty = x1 - x0 < 1e-6 && y1 - y0 < 1e-6;
        if let Some(r) = &mut cur {
            if empty {
                r.text.push(c.ch);
                continue;
            }
            let h = (r.y1 - r.y0).max(1e-3);
            let same_line = ((y0 + y1) / 2.0 - (r.y0 + r.y1) / 2.0).abs() < 0.6 * h;
            let gap = x0 - r.x1;
            if same_line && gap < 2.5 * h && gap > -2.0 * h {
                r.text.push(c.ch);
                r.x0 = r.x0.min(x0);
                r.x1 = r.x1.max(x1);
                r.y0 = r.y0.min(y0);
                r.y1 = r.y1.max(y1);
                continue;
            }
            flush(&mut cur, &mut out);
        }
        if empty {
            continue;
        }
        cur = Some(Run {
            text: c.ch.to_string(),
            x0,
            y0,
            x1,
            y1,
        });
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use papyrine_ipc::CharBoxInfo;

    fn chars(s: &str, x: f64, bottom: f64) -> Vec<CharBoxInfo> {
        s.chars()
            .enumerate()
            .map(|(i, ch)| CharBoxInfo {
                ch,
                left: x + i as f64 * 6.0,
                right: x + i as f64 * 6.0 + 6.0,
                bottom,
                top: bottom + 10.0,
            })
            .collect()
    }

    #[test]
    fn buckets_are_sqrt2_steps() {
        assert_eq!(bucket_for_scale(1.0), 0);
        assert_eq!(bucket_for_scale(2.0), 2);
        assert_eq!(bucket_for_scale(std::f64::consts::SQRT_2), 1);
        assert_eq!(bucket_for_scale(0.0), 0);
        assert_eq!(bucket_for_scale(1e9), MAX_BUCKET);
        assert_eq!(bucket_for_scale(1e-9), MIN_BUCKET);
    }

    #[test]
    fn upright_text_becomes_top_left_runs() {
        let mut cs = chars("Hello", 72.0, 700.0);
        cs.push(CharBoxInfo {
            ch: '\n',
            left: 0.0,
            right: 0.0,
            bottom: 0.0,
            top: 0.0,
        });
        cs.extend(chars("World", 72.0, 680.0));
        let t = PageTextInfo {
            page: 0,
            text: "Hello\nWorld".into(),
            chars: cs,
        };
        let runs = runs_from(&t, 612.0, 792.0, 0);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "Hello");
        assert!((runs[0].x - 72.0).abs() < 1e-9);
        assert!((runs[0].y - (792.0 - 710.0)).abs() < 1e-9);
        assert!((runs[0].width - 30.0).abs() < 1e-9);
        assert!(runs[1].y > runs[0].y, "second line is lower on the page");
    }

    #[test]
    fn rotation_maps_corners() {
        let t = PageTextInfo {
            page: 0,
            text: "A".into(),
            chars: vec![CharBoxInfo {
                ch: 'A',
                left: 10.0,
                right: 20.0,
                bottom: 30.0,
                top: 40.0,
            }],
        };
        // Unrotated page 100 x 200 (user space); displayed sizes swap for 90/270.
        let r0 = &runs_from(&t, 100.0, 200.0, 0)[0];
        assert_eq!((r0.x, r0.y), (10.0, 200.0 - 40.0));
        let r90 = &runs_from(&t, 200.0, 100.0, 90)[0];
        assert_eq!(
            (r90.x, r90.y, r90.width, r90.height),
            (30.0, 10.0, 10.0, 10.0)
        );
        let r180 = &runs_from(&t, 100.0, 200.0, 180)[0];
        assert_eq!((r180.x, r180.y), (100.0 - 20.0, 30.0));
        let r270 = &runs_from(&t, 200.0, 100.0, 270)[0];
        assert_eq!((r270.x, r270.y), (200.0 - 40.0, 100.0 - 20.0));
    }

    #[test]
    fn hit_offsets_are_utf16() {
        let h = papyrine_ipc::SearchHit {
            page: 2,
            quads: vec![],
            context: "caf\u{e9} Needle here".into(),
        };
        let m = map_hit(&h, "needle", false);
        assert_eq!((m.page, m.match_start, m.match_length), (2, 5, 6));
    }
}
