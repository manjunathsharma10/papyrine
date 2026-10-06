//! JSON shapes of `src/ipc/contract.ts`. Field names are camelCase on the wire.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentMeta {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub producer: String,
    pub pdf_version: String,
    pub encrypted: bool,
    pub creator: String,
    /// ISO 8601; empty when absent.
    pub created: String,
    pub modified: String,
    /// Size of the file on disk (0 for unsaved bytes).
    pub file_size: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormKind {
    #[default]
    None,
    Acroform,
    XfaStatic,
    XfaDynamic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    pub doc_id: String,
    pub name: String,
    pub path: Option<String>,
    pub page_count: usize,
    pub pages: Vec<PageInfo>,
    pub meta: DocumentMeta,
    pub repaired: bool,
    pub revision: u64,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub signed: bool,
    pub form_kind: FormKind,
    pub form_scripts: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    pub title: String,
    pub page: Option<usize>,
    pub children: Vec<OutlineNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRun {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageText {
    pub page: usize,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub diacritic_insensitive: bool,
    #[serde(default)]
    pub include_comments: bool,
    #[serde(default)]
    pub include_form_values: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: usize,
    pub snippet: String,
    pub match_start: usize,
    pub match_length: usize,
    /// "text" | "comment" | "form"
    pub source: String,
    /// Highlight quads in displayed page points (x1,y1 .. x4,y4).
    pub quads: Vec<[f64; 8]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EngineCommand {
    SetMetadata {
        fields: MetadataFields,
    },
    RotatePages {
        pages: Vec<usize>,
        degrees: i32,
    },
    DeletePages {
        pages: Vec<usize>,
    },
    MovePages {
        pages: Vec<usize>,
        to: usize,
    },
    DuplicatePages {
        pages: Vec<usize>,
    },
    InsertBlankPage {
        at: usize,
        width: Option<f64>,
        height: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    InsertPages {
        source_path: String,
        pages: Option<Vec<usize>>,
        at: usize,
    },
    /// Annotation, form and flat-fill commands: routed once the engine registers them.
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetadataFields {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub info: DocumentInfo,
    pub invalidated_pages: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SaveMode {
    #[default]
    Default,
    Optimized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaveChoice {
    AppendAnyway,
    SaveCopy,
    OptimizeAndInvalidate,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOptions {
    /// Omit to save in place; set for Save As.
    pub path: Option<String>,
    #[serde(default)]
    pub mode: SaveMode,
    pub choice: Option<SaveChoice>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // wire type, built once per save
pub enum SaveReport {
    Saved {
        info: DocumentInfo,
        #[serde(skip_serializing_if = "Option::is_none")]
        suggest_optimize: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rewritten_because: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        history_truncated: Option<bool>,
    },
    DecisionNeeded {
        reason: String,
        choices: Vec<SaveChoice>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryEntry {
    pub id: String,
    pub name: String,
    pub path: Option<String>,
    pub last_activity: String,
    /// "restorable" | "original-changed"
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<UnfinishedTitle>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnfinishedTitle {
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedFile {
    pub path: String,
    pub name: String,
    pub page_count: usize,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OpenSourceJson {
    Path {
        path: String,
    },
    /// Only produced by the host (open-requested); bytes arrive through a raw channel.
    Bytes {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub name: String,
    pub path: String,
}

/// Events pushed to the UI. The first eight are `HostEvent` in contract.ts; the rest are
/// additive (the UI ignores unknown types until it adopts them).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)] // wire type, built once per event
pub enum HostEvent {
    #[serde(rename_all = "camelCase")]
    DocumentChanged {
        info: DocumentInfo,
        invalidated_pages: Vec<usize>,
    },
    #[serde(rename_all = "camelCase")]
    SearchHit { job_id: String, hit: SearchHit },
    #[serde(rename_all = "camelCase")]
    JobProgress {
        job_id: String,
        done: u64,
        total: u64,
        finished: bool,
    },
    #[serde(rename_all = "camelCase")]
    FileChangedOnDisk { doc_id: String },
    #[serde(rename_all = "camelCase")]
    ActionPrompt {
        doc_id: String,
        prompt_id: String,
        kind: String,
        target: String,
    },
    #[serde(rename_all = "camelCase")]
    OpenRequested { sources: Vec<OpenSourceJson> },
    #[serde(rename_all = "camelCase")]
    EngineRestarted {
        doc_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped_action: Option<String>,
    },
    /// A short user-visible message ("Engine restarted; no changes lost.").
    #[serde(rename_all = "camelCase")]
    Notice {
        level: String,
        code: String,
        message: String,
        doc_id: Option<String>,
    },
    /// Unsaved work from a previous run was found.
    #[serde(rename_all = "camelCase")]
    RecoveryAvailable { entries: Vec<RecoveryEntry> },
}

impl HostEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            HostEvent::DocumentChanged { .. } => "document-changed",
            HostEvent::SearchHit { .. } => "search-hit",
            HostEvent::JobProgress { .. } => "job-progress",
            HostEvent::FileChangedOnDisk { .. } => "file-changed-on-disk",
            HostEvent::ActionPrompt { .. } => "action-prompt",
            HostEvent::OpenRequested { .. } => "open-requested",
            HostEvent::EngineRestarted { .. } => "engine-restarted",
            HostEvent::Notice { .. } => "notice",
            HostEvent::RecoveryAvailable { .. } => "recovery-available",
        }
    }
}

/// Where host events go: the Tauri window, a WebSocket client or a test collector.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: HostEvent);
}

/// Collects events (tests, headless runs).
#[derive(Default)]
pub struct CollectSink(pub std::sync::Mutex<Vec<HostEvent>>);

impl EventSink for CollectSink {
    fn emit(&self, event: HostEvent) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(event);
    }
}

impl CollectSink {
    pub fn take(&self) -> Vec<HostEvent> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()))
    }
    pub fn snapshot(&self) -> Vec<HostEvent> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Convenience for dispatch code: any JSON value.
pub type Json = Value;
