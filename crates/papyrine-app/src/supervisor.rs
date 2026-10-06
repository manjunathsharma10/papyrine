//! Restart helpers: a restart policy for crashing children, and renderer snapshot reload.
//!
//! Engine restart (reopen + journal replay) lives in [`crate::host::EngineHost::restart_engine`]
//! because it needs the journals. The renderer needs no journal: it reloads the current
//! snapshot, `base ‖ sections`, which the host keeps (the base file plus the section bytes the
//! engine delivered).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use papyrine_ipc::{
    DocId, DocSource, ErrorCode, IpcError, PixelDest, RenderRequest, RenderResponse, SharedRegion,
};

use crate::spawn::{RenderChild, SpawnConfig, spawn_renderer};

/// Allow a few restarts in a window, then give up (no crash loop).
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    pub max: usize,
    pub window: Duration,
    deaths: VecDeque<Instant>,
}

impl RestartPolicy {
    pub fn new(max: usize, window: Duration) -> Self {
        RestartPolicy {
            max,
            window,
            deaths: VecDeque::new(),
        }
    }

    /// Record a death; `true` while another restart is allowed.
    pub fn record_death(&mut self) -> bool {
        let now = Instant::now();
        self.deaths.push_back(now);
        while self
            .deaths
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.window)
        {
            self.deaths.pop_front();
        }
        self.deaths.len() <= self.max
    }
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy::new(5, Duration::from_secs(60))
    }
}

/// What the renderer needs to hold a document again.
pub struct RenderDoc {
    pub doc: DocId,
    pub base: DocSource,
    pub sections: Vec<DocSource>,
    pub password: Option<String>,
}

/// The tile pool the renderer writes pixels into (re-registered after a restart).
pub struct TilePool {
    pub pool: SharedRegion,
    pub slot_bytes: u64,
    pub slots: u32,
}

/// Start a fresh renderer and reload `docs` into it from their current snapshots.
pub fn restart_renderer(
    cfg: &SpawnConfig,
    pool: Option<TilePool>,
    docs: Vec<RenderDoc>,
) -> Result<RenderChild, IpcError> {
    let child = spawn_renderer(cfg)?;
    if let Some(p) = pool {
        match child.client.call_blocking(RenderRequest::SetTilePool {
            pool: p.pool,
            slot_bytes: p.slot_bytes,
            slots: p.slots,
        })? {
            RenderResponse::PoolReady => {}
            r => return Err(IpcError::internal(format!("unexpected reply {r:?}"))),
        }
    }
    for d in docs {
        match child.client.call_blocking(RenderRequest::Open {
            doc: d.doc,
            base: d.base,
            sections: d.sections,
            password: d.password,
        })? {
            RenderResponse::Opened { .. } => {}
            r => return Err(IpcError::internal(format!("unexpected reply {r:?}"))),
        }
    }
    Ok(child)
}

/// Did this call fail because the child died (as opposed to a request error)?
pub fn child_died(e: &IpcError) -> bool {
    e.code == ErrorCode::ChildCrashed
}

/// Render a page preview inline (convenience for tests and tools).
pub fn preview_inline(
    child: &RenderChild,
    doc: DocId,
    page: u32,
    max_edge: u32,
) -> Result<papyrine_ipc::PixelsInfo, IpcError> {
    match child.client.call_blocking(RenderRequest::RenderPreview {
        doc,
        page,
        max_edge,
        dest: PixelDest::Inline,
    })? {
        RenderResponse::Pixels(p) => Ok(p),
        r => Err(IpcError::internal(format!("unexpected reply {r:?}"))),
    }
}
