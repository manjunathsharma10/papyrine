//! The engine's request/response set, carried by `papyrine-ipc`'s transport.
//!
//! `papyrine_ipc::EngineRequest` predates the document actor and cannot express saves,
//! checkpoints, journal replay, snapshot compaction or the model queries, so the engine role
//! speaks this richer set (`Request` / `Response`). Shared payload types (`DocId`, `DocSource`,
//! `BlobRef`, `CommandRequest`, `ChangeSetSummary`, `AfterImage`, `SharedRegion`) are reused from
//! `papyrine-ipc`. Everything here is plain data (no skipped fields, no internal tags) so it
//! round-trips through postcard.
//!
//! Output bytes never cross the sandbox as paths the engine reads: they come back as a
//! [`Delivery`], which is shared memory the host allocated, inline bytes, or a file in the
//! engine's own temp directory that the host collects.

use papyrine_ipc::{AfterImage, BlobRef, ChangeSetSummary, CommandRequest, DocId, DocSource};
use papyrine_ipc::{IpcError, SharedRegion};
use serde::{Deserialize, Serialize};

/// Inline delivery limit; bigger outputs go to a temp file.
pub const INLINE_LIMIT: usize = 4 << 20;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Ping {
        nonce: u64,
    },
    /// Load a document. Replies with [`Response::Opened`].
    Open {
        doc: DocId,
        source: DocSource,
        password: Option<String>,
        params: OpenParams,
    },
    Close {
        doc: DocId,
    },
    /// Apply a command. `out_section` receives the new snapshot section when it fits.
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
    /// Apply journalled after-images directly (never re-runs command logic). Used after an
    /// engine restart or crash recovery: `Open` the original, then `Replay`.
    Replay {
        doc: DocId,
        commits: Vec<ReplayCommit>,
        out_section: Option<SharedRegion>,
    },
    /// Produce the bytes of a save. The host does the atomic replace, then sends
    /// [`Request::SaveCommitted`].
    Save {
        doc: DocId,
        mode: SaveMode,
    },
    SaveCommitted {
        doc: DocId,
        token: u64,
        /// The file as saved. Required for optimized saves (the engine reopens it, because
        /// object numbers changed).
        saved: Option<DocSource>,
    },
    /// ID-preserving full write of the current state (journal checkpoint).
    Checkpoint {
        doc: DocId,
    },
    /// Run snapshot compaction now (the host sends this when idle after `advice`).
    Compact {
        doc: DocId,
        level: CompactLevel,
    },
    Query {
        doc: DocId,
        query: Query,
    },
    /// Per-object fingerprints of the whole document, for crash-recovery verification.
    Digest {
        doc: DocId,
        per_object: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenParams {
    /// Skip the per-page size list in the open metadata (the renderer reports sizes too).
    pub skip_pages: bool,
    /// Where the history spill and large outputs may go; `None` = the engine temp dir.
    pub spill_budget_bytes: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReplayCommit {
    pub seq: u64,
    pub command: String,
    pub after_images: Vec<AfterImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactLevel {
    /// Merge all sections into one.
    L1,
    /// ID-preserving full write becomes the new render base.
    L2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveMode {
    /// Follow the save policy: incremental when safe, an optimized rewrite for a repaired file,
    /// `DecisionNeeded` for a signed and damaged one unless `break_signatures`.
    Policy { break_signatures: bool },
    /// Always a qpdf optimized rewrite (object streams, compression, garbage collection).
    Optimized { break_signatures: bool },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Pong { nonce: u64 },
    Opened(Box<DocSummary>),
    Closed,
    Committed(Box<Committed>),
    Replayed(Box<Replayed>),
    SaveDecision(SaveDecision),
    SaveReady(Box<SavePayload>),
    SaveFinished(SaveFinished),
    Checkpointed(OutputFile),
    Compacted(SnapshotUpdate),
    Query(Box<QueryResult>),
    Digest(DigestInfo),
}

/// A file the engine wrote in its own temp directory; the host collects (and deletes) it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputFile {
    pub path: String,
    pub len: u64,
}

/// How a block of output bytes reaches the host.
#[derive(Debug, Serialize, Deserialize)]
pub enum Delivery {
    /// Written at offset 0 of the region the host passed with the request.
    Shared {
        len: u64,
    },
    Inline(Vec<u8>),
    File(OutputFile),
}

/// What the renderer must do after a state change.
#[derive(Debug, Serialize, Deserialize)]
pub enum SnapshotAction {
    /// Nothing changed in the PDF bytes.
    Unchanged,
    /// L0: append this section to the current list.
    Append(Delivery),
    /// L1: the list becomes exactly this one section.
    ReplaceSections(Delivery),
    /// L2 (or a reopen after an optimized save): a new base file, no sections.
    NewBase(Option<OutputFile>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotUpdate {
    pub action: SnapshotAction,
    /// Increases with every change of the snapshot; the host can detect lost updates.
    pub epoch: u64,
    /// Sections after this update and their total bytes.
    pub sections: u32,
    pub section_bytes: u64,
    /// Set when the engine wants an L2 compaction when the host is next idle.
    pub advice: Option<CompactLevel>,
}

/// Result of Execute / Undo / Redo.
#[derive(Debug, Serialize, Deserialize)]
pub struct Committed {
    pub summary: ChangeSetSummary,
    /// Serialized objects for the journal's `Commit` record.
    pub after_images: Vec<AfterImage>,
    /// Object numbers allocated by the command.
    pub created: Vec<(u32, u16)>,
    pub snapshot: SnapshotUpdate,
    /// Localized description key of the step (menu text "Undo Rotate pages").
    pub label_key: String,
    pub label_args: Vec<(String, String)>,
    /// Wall time inside the actor, microseconds.
    pub engine_micros: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Replayed {
    pub applied: u32,
    pub snapshot: SnapshotUpdate,
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty_pages: Vec<u32>,
    pub structure_changed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SaveDecision {
    /// The file is signed and damaged. Ask: rewrite anyway (signatures become invalid) or
    /// save a copy.
    SignedAndDamaged { reason: String },
    /// An optimized rewrite was requested for a signed file; it invalidates the signatures.
    SignedRewrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveKindDto {
    Incremental,
    Optimized,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavePayload {
    pub token: u64,
    pub kind: SaveKindDto,
    /// Incremental: bytes to append to the current on-disk file (`base_len` bytes long).
    /// Optimized: the complete new file.
    pub bytes: Delivery,
    pub base_len: u64,
    pub total_len: u64,
    pub sections: u32,
    pub page_count: u32,
    /// One-time "Save optimized" suggestion.
    pub suggest_optimize: Option<String>,
    /// The save rewrote the file; undo history will be dropped when it commits.
    pub history_will_reset: bool,
    /// Nothing changed since the last save; `bytes` is empty and the file need not be touched.
    pub unchanged: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveFinished {
    pub snapshot: SnapshotUpdate,
    pub history_reset: bool,
    pub notice: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DigestInfo {
    pub objects: u32,
    /// BLAKE3 over every object's id and fingerprint, sorted by id.
    pub digest: [u8; 32],
    /// `(num, generation, blake3-of-fingerprint)` when requested.
    pub per_object: Vec<(u32, u16, [u8; 32])>,
}

// ---------------------------------------------------------------- queries

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Query {
    Summary,
    Pages { from: u32, count: u32 },
    Outline,
    Labels,
    Fields,
    Annotations { page: u32 },
    Info,
    Security,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum QueryResult {
    Summary(Box<DocSummary>),
    Pages(Vec<PageDto>),
    Outline(Vec<OutlineDto>),
    Labels(Vec<LabelRangeDto>),
    Fields(Vec<FieldDto>),
    Annotations(Vec<AnnotDto>),
    Info(Box<InfoDto>),
    Security(SecurityDto),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageDto {
    pub index: u32,
    pub width_pt: f64,
    pub height_pt: f64,
    pub rotation: u16,
    pub label: String,
    pub annotation_count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineDto {
    pub title: String,
    /// Zero-based target page in this document.
    pub page: Option<u32>,
    pub open: bool,
    pub children: Vec<OutlineDto>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelRangeDto {
    pub start_index: u32,
    pub style: Option<String>,
    pub prefix: String,
    pub first_number: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldDto {
    pub id: (u32, u16),
    pub name: String,
    pub kind: String,
    /// Text value, or the state name of a checkbox / radio.
    pub value: Option<String>,
    /// Selected values of a list box or multi-select.
    pub values: Vec<String>,
    pub flags: u32,
    pub read_only: bool,
    pub required: bool,
    pub max_len: Option<u32>,
    /// Zero-based pages carrying a widget of this field.
    pub pages: Vec<u32>,
    /// `(export, display)` choices.
    pub options: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnnotDto {
    pub id: Option<(u32, u16)>,
    pub page: u32,
    pub index: u32,
    pub subtype: String,
    pub rect: [f64; 4],
    pub flags: u32,
    pub contents: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub name: Option<String>,
    pub modified: Option<String>,
    pub color: Option<Vec<f64>>,
    pub opacity: f64,
    pub has_appearance: bool,
    pub in_reply_to: Option<(u32, u16)>,
    pub uri: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InfoDto {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
    pub custom: Vec<(String, String)>,
    pub version: String,
    pub linearized: bool,
    pub page_layout: Option<String>,
    pub page_mode: Option<String>,
    pub language: Option<String>,
    pub tagged: bool,
    pub has_xmp: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermsDto {
    pub print: bool,
    pub print_high_quality: bool,
    pub copy: bool,
    pub accessibility: bool,
    pub modify_contents: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityDto {
    pub encrypted: bool,
    pub algorithm: Option<String>,
    pub owner_authenticated: bool,
    pub declared: PermsDto,
    pub effective: PermsDto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormSummary {
    pub has_acroform: bool,
    pub field_count: u32,
    pub widget_count: u32,
    pub signature_fields: u32,
    pub need_appearances: bool,
    /// `none`, `static` or `dynamic`.
    pub xfa: String,
    pub needs_rendering: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotSummary {
    pub total: u32,
    pub pages_with_annotations: u32,
    pub by_subtype: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureSummaryDto {
    pub signed: bool,
    pub signed_fields: u32,
    pub unsigned_fields: u32,
    pub certified: bool,
    pub requires_incremental_save: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RenderBase {
    /// The renderer maps the file it already has (the original).
    Original,
    /// qpdf had to repair the file: render from this ID-preserving full write instead, so both
    /// processes see the same object numbers.
    Repaired(OutputFile),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DocSummary {
    pub doc: DocId,
    pub page_count: u32,
    pub pages: Vec<PageDto>,
    pub outline: Vec<OutlineDto>,
    pub labels: Vec<LabelRangeDto>,
    pub form: FormSummary,
    pub annotations: AnnotSummary,
    pub info: InfoDto,
    pub security: SecurityDto,
    pub signatures: SignatureSummaryDto,
    pub repaired: bool,
    /// The repair log, one line per entry.
    pub repair_log: Vec<String>,
    pub render_base: RenderBase,
    pub can_undo: bool,
    pub can_redo: bool,
    pub open_micros: u64,
}

impl From<papyrine_cos::Error> for EngineError {
    fn from(e: papyrine_cos::Error) -> Self {
        EngineError::Cos(e)
    }
}

/// Internal error type; converted to [`IpcError`] at the protocol boundary.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Cos(papyrine_cos::Error),
    #[error(transparent)]
    Ops(#[from] papyrine_ops::Error),
    #[error(transparent)]
    Writer(#[from] papyrine_writer::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error("{0}")]
    Invalid(String),
    #[error("no such document")]
    NoDoc,
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<EngineError> for IpcError {
    fn from(e: EngineError) -> Self {
        use papyrine_ipc::ErrorCode;
        if let EngineError::Ipc(i) = e {
            return i;
        }
        let code = match &e {
            EngineError::Cancelled => ErrorCode::Cancelled,
            EngineError::Invalid(_) | EngineError::Ops(papyrine_ops::Error::InvalidParams(_)) => {
                ErrorCode::InvalidRequest
            }
            EngineError::Ops(papyrine_ops::Error::UnknownCommand(_)) => ErrorCode::Unsupported,
            EngineError::NoDoc => ErrorCode::NotFound,
            EngineError::Io(_) => ErrorCode::Io,
            EngineError::Cos(c) | EngineError::Ops(papyrine_ops::Error::Cos(c)) => cos_code(c),
            EngineError::Writer(papyrine_writer::Error::Cos(c)) => cos_code(c),
            EngineError::Writer(papyrine_writer::Error::Cancelled) => ErrorCode::Cancelled,
            EngineError::Writer(_) => ErrorCode::Io,
            _ => ErrorCode::Internal,
        };
        IpcError::new(code, e.to_string())
    }
}

fn cos_code(c: &papyrine_cos::Error) -> papyrine_ipc::ErrorCode {
    use papyrine_cos::Error as E;
    use papyrine_ipc::ErrorCode;
    match c {
        _ if c.is_cancelled() => ErrorCode::Cancelled,
        E::InvalidPassword => ErrorCode::Encrypted,
        E::Damaged { .. } => ErrorCode::Corrupt,
        E::Unsupported(_) => ErrorCode::Unsupported,
        E::Io(_) => ErrorCode::Io,
        E::Range(_) | E::Type(_) => ErrorCode::InvalidRequest,
        _ => ErrorCode::Internal,
    }
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;
