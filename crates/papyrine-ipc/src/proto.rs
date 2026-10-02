//! The protocol: ids, handshake, framing envelopes and the request/response
//! sets for the engine and the renderer (ARCHITECTURE §4.9).
//!
//! Types that the web UI also sees derive [`ts_rs::TS`]; `tests/ts_bindings.rs`
//! writes them to `apps/desktop/src/ipc/generated/`. Types that carry OS
//! handles or shared memory (`Handle`, `SharedRegion`) are host/child only.

use crate::handle::Handle;
use crate::shm::{MappedFile, SharedBytes, SharedRegion};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

/// Current protocol version. Bump on any wire-incompatible change; keep
/// [`PROTOCOL_MIN`] at the oldest version this build can still speak.
pub const PROTOCOL_VERSION: u16 = 1;
pub const PROTOCOL_MIN: u16 = 1;

macro_rules! wire_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        #[ts(type = "number")]
        pub struct $name(pub u64);
    };
}

wire_id!(
    /// Correlates a response with its request.
    RequestId
);
wire_id!(
    /// A cancellable long-running job (search, save, OCR ...).
    JobId
);
wire_id!(
    /// An open document inside one child process.
    DocId
);

impl From<RequestId> for papyrine_core::RequestId {
    fn from(v: RequestId) -> Self {
        papyrine_core::RequestId(v.0)
    }
}
impl From<papyrine_core::RequestId> for RequestId {
    fn from(v: papyrine_core::RequestId) -> Self {
        RequestId(v.0)
    }
}
impl From<JobId> for papyrine_core::JobId {
    fn from(v: JobId) -> Self {
        papyrine_core::JobId(v.0)
    }
}
impl From<papyrine_core::JobId> for JobId {
    fn from(v: papyrine_core::JobId) -> Self {
        JobId(v.0)
    }
}

// ---------------------------------------------------------------- handshake

/// What a child process is for. `Other` carries a component helper's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum Role {
    Engine,
    Renderer,
    Other(String),
}

impl Role {
    pub fn as_arg(&self) -> String {
        match self {
            Role::Engine => "engine".into(),
            Role::Renderer => "renderer".into(),
            Role::Other(s) => format!("other:{s}"),
        }
    }

    pub fn from_arg(s: &str) -> Option<Role> {
        match s {
            "engine" => Some(Role::Engine),
            "renderer" => Some(Role::Renderer),
            _ => s.strip_prefix("other:").map(|n| Role::Other(n.to_string())),
        }
    }
}

/// First message in each direction. Each side states the protocol range it
/// speaks; the highest common version is used for the session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Hello {
    pub min: u16,
    pub max: u16,
    pub role: Role,
    #[ts(type = "number")]
    pub pid: u32,
    /// Free-form build identification (for diagnostics only).
    pub build: String,
}

impl Hello {
    pub fn new(role: Role) -> Self {
        Hello {
            min: PROTOCOL_MIN,
            max: PROTOCOL_VERSION,
            role,
            pid: std::process::id(),
            build: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// The highest version both sides speak, if any.
pub fn negotiate(local: (u16, u16), remote: (u16, u16)) -> Option<u16> {
    let hi = local.1.min(remote.1);
    let lo = local.0.max(remote.0);
    (lo <= hi).then_some(hi)
}

// ------------------------------------------------------------------- errors

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ErrorCode {
    Cancelled,
    InvalidRequest,
    NotFound,
    Unsupported,
    /// The document needs a password (or the one given is wrong).
    Encrypted,
    Corrupt,
    Io,
    Internal,
    /// Synthesised by the host when the child died or the channel broke.
    ChildCrashed,
    VersionMismatch,
    /// A shared-memory output region was too small; `message` has the size needed.
    OutputTooSmall,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, m)
    }
    pub fn invalid(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, m)
    }
    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "cancelled")
    }
    pub fn is_cancelled(&self) -> bool {
        self.code == ErrorCode::Cancelled
    }
}

// --------------------------------------------------------------- envelopes

/// Host to child.
#[derive(Debug, Serialize, Deserialize)]
pub enum HostToChild<Q> {
    Hello(Hello),
    /// `job` is `Some` for long-running work: progress is reported against it
    /// and it can be cancelled with [`HostToChild::Cancel`].
    Request {
        id: RequestId,
        job: Option<JobId>,
        body: Q,
    },
    Cancel {
        job: JobId,
    },
    /// Finish the current request, then exit cleanly.
    Shutdown,
}

/// Child to host.
#[derive(Debug, Serialize, Deserialize)]
pub enum ChildToHost<R> {
    Hello(Hello),
    Response {
        id: RequestId,
        result: Result<R, IpcError>,
    },
    Progress(Progress),
    /// An intermediate result of a job (for example search hits found so far).
    Partial {
        job: JobId,
        value: R,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Progress {
    pub job: JobId,
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
    pub message: String,
}

// ------------------------------------------------------- shared payloads

/// How a document's bytes reach a child that may not open files itself.
#[derive(Debug, Serialize, Deserialize)]
pub enum DocSource {
    /// A read-only handle to the file; the child maps it. The cheap, no-copy path.
    File { handle: Handle, len: u64 },
    /// Bytes in shared memory (for example an in-memory snapshot).
    Shared { region: SharedRegion, len: u64 },
    /// Fallback when handle passing is unavailable: the bytes themselves.
    Bytes(Vec<u8>),
}

/// Bytes that can back a PDF parser without copying.
pub type SourceBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

impl DocSource {
    pub fn len(&self) -> u64 {
        match self {
            DocSource::File { len, .. } | DocSource::Shared { len, .. } => *len,
            DocSource::Bytes(b) => b.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Materialise as shared immutable bytes (mmap / shared pages / the vec).
    pub fn into_bytes(self) -> Result<SourceBytes, IpcError> {
        match self {
            DocSource::File { handle, len } => MappedFile::map(&handle, len)
                .map(|m| Arc::new(m) as SourceBytes)
                .map_err(|e| IpcError::new(ErrorCode::Io, format!("mapping document: {e}"))),
            DocSource::Shared { region, len } => {
                let len = usize::try_from(len).map_err(|_| IpcError::invalid("length"))?;
                if len > region.len() {
                    return Err(IpcError::invalid("source longer than its region"));
                }
                Ok(Arc::new(SharedBytes { region, len }))
            }
            DocSource::Bytes(v) => Ok(Arc::new(v)),
        }
    }
}

/// Opaque binary argument of a command (an image to place, a font ...).
#[derive(Debug, Serialize, Deserialize)]
pub enum BlobRef {
    Inline(Vec<u8>),
    Shared {
        region: SharedRegion,
        offset: u64,
        len: u64,
    },
    File {
        handle: Handle,
        len: u64,
    },
}

/// The UI-visible half of a command: the engine looks `name` up in its
/// command registry and parses `params_json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CommandRequest {
    pub name: String,
    pub params_json: String,
}

/// Summary of a committed ChangeSet (the UI re-requests only invalidated tiles).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ChangeSetSummary {
    pub label: String,
    pub dirty_pages: Vec<u32>,
    pub structure_changed: bool,
    pub objects_changed: u32,
    pub objects_added: u32,
    pub objects_removed: u32,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// Serialized new/changed objects, enough for the journal and for undo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfterImage {
    pub obj: u32,
    pub generation: u16,
    /// Object body in PDF syntax; `None` means the object was deleted.
    pub body: Option<Vec<u8>>,
}

/// Where an appended snapshot section was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SectionInfo {
    #[ts(type = "number")]
    pub generation: u64,
    #[ts(type = "number")]
    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct DocumentInfo {
    pub doc: DocId,
    pub page_count: u32,
    pub encrypted: bool,
    pub repaired: bool,
    pub title: Option<String>,
    /// qpdf warnings (the RepairLog) rendered one line each.
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct PageSizeInfo {
    pub width_pt: f32,
    pub height_pt: f32,
    pub rotation: u16,
}

/// Four corners of a highlight quad, page space (points, origin bottom-left):
/// x1,y1 .. x4,y4 in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Quad(pub [f64; 8]);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SearchHit {
    pub page: u32,
    pub quads: Vec<Quad>,
    pub context: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub max_hits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct CharBoxInfo {
    pub ch: char,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct PageTextInfo {
    pub page: u32,
    pub text: String,
    pub chars: Vec<CharBoxInfo>,
}

/// Where rendered pixels go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum PixelDest {
    /// A slot of the tile pool registered with [`RenderRequest::SetTilePool`].
    Slot(u32),
    /// Return the pixels inside the response (fallback; large messages).
    Inline,
}

/// Result of a tile or preview render. Pixels are RGBA8, row-major, opaque.
#[derive(Debug, Serialize, Deserialize)]
pub struct PixelsInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub slot: Option<u32>,
    /// Present when the request asked for [`PixelDest::Inline`].
    pub inline: Option<Vec<u8>>,
}

// -------------------------------------------------------------- engine set

#[derive(Debug, Serialize, Deserialize)]
pub enum EngineRequest {
    Ping {
        nonce: u64,
    },
    Open {
        doc: DocId,
        source: DocSource,
        password: Option<String>,
    },
    Close {
        doc: DocId,
    },
    /// Run a command. `out_section`, when given, receives the new snapshot
    /// section's bytes (host-allocated so it works under every sandbox).
    Execute {
        doc: DocId,
        command: CommandRequest,
        blobs: Vec<BlobRef>,
        out_section: Option<SharedRegion>,
    },
    Undo {
        doc: DocId,
        out_section: Option<SharedRegion>,
    },
    Redo {
        doc: DocId,
        out_section: Option<SharedRegion>,
    },
    /// Long job: streams `Partial(Hits)` and finishes with `Hits`.
    Search {
        doc: DocId,
        query: SearchQuery,
    },
    PageText {
        doc: DocId,
        page: u32,
    },
    /// Serialize the whole document into `out` (the host does the atomic replace).
    Write {
        doc: DocId,
        out: SharedRegion,
        options_json: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum EngineResponse {
    Pong {
        nonce: u64,
    },
    Opened(DocumentInfo),
    Closed,
    Executed {
        summary: ChangeSetSummary,
        after_images: Vec<AfterImage>,
        section: Option<SectionInfo>,
    },
    Hits {
        hits: Vec<SearchHit>,
        complete: bool,
    },
    Text(PageTextInfo),
    Written {
        len: u64,
    },
}

// ------------------------------------------------------------ renderer set

#[derive(Debug, Serialize, Deserialize)]
pub enum RenderRequest {
    Ping {
        nonce: u64,
    },
    /// Register the shared tile pool: `slots` slots of `slot_bytes` each.
    SetTilePool {
        pool: SharedRegion,
        slot_bytes: u64,
        slots: u32,
    },
    Open {
        doc: DocId,
        base: DocSource,
        sections: Vec<DocSource>,
        password: Option<String>,
    },
    /// Re-open over the same base with a new section list (after an edit).
    Reopen {
        doc: DocId,
        sections: Vec<DocSource>,
    },
    Close {
        doc: DocId,
    },
    PageSizes {
        doc: DocId,
    },
    RenderTile {
        doc: DocId,
        page: u32,
        bucket: i32,
        tile_x: u32,
        tile_y: u32,
        dest: PixelDest,
    },
    RenderPreview {
        doc: DocId,
        page: u32,
        max_edge: u32,
        dest: PixelDest,
    },
    PageText {
        doc: DocId,
        page: u32,
    },
    Search {
        doc: DocId,
        query: SearchQuery,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RenderResponse {
    Pong {
        nonce: u64,
    },
    PoolReady,
    Opened {
        doc: DocId,
        page_count: u32,
    },
    Closed,
    PageSizes(Vec<PageSizeInfo>),
    Pixels(PixelsInfo),
    Text(PageTextInfo),
    Hits {
        hits: Vec<SearchHit>,
        complete: bool,
    },
}
