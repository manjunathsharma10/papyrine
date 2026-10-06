//! The error type that crosses the UI boundary: `{ code, message }`, mirrored by
//! `HostError` in `src/ipc/contract.ts`.

use papyrine_ipc::{ErrorCode, IpcError};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Code {
    NotFound,
    Cancelled,
    Encrypted,
    Corrupt,
    Io,
    Internal,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct HostErr {
    pub code: Code,
    pub message: String,
    /// Machine-readable reason for errors the UI can act on (`signed-and-damaged`,
    /// `signed-rewrite`: ask the user, then retry with `breakSignatures`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The protocol error this came from, when it came from a child.
    #[serde(skip)]
    pub ipc: Option<ErrorCode>,
}

pub type Result<T> = std::result::Result<T, HostErr>;

impl HostErr {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            ipc: None,
        }
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(Code::Internal, m)
    }
    pub fn io(m: impl Into<String>) -> Self {
        Self::new(Code::Io, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(Code::NotFound, m)
    }
    pub fn cancelled() -> Self {
        Self::new(Code::Cancelled, "cancelled")
    }
    pub fn is_cancelled(&self) -> bool {
        self.code == Code::Cancelled
    }
}

impl From<IpcError> for HostErr {
    fn from(e: IpcError) -> Self {
        let code = match e.code {
            ErrorCode::Cancelled => Code::Cancelled,
            ErrorCode::NotFound => Code::NotFound,
            ErrorCode::Encrypted => Code::Encrypted,
            ErrorCode::Corrupt => Code::Corrupt,
            ErrorCode::Io => Code::Io,
            _ => Code::Internal,
        };
        let mut h = Self::new(code, e.message);
        h.ipc = Some(e.code);
        h
    }
}

impl From<std::io::Error> for HostErr {
    fn from(e: std::io::Error) -> Self {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            Code::NotFound
        } else {
            Code::Io
        };
        Self::new(code, e.to_string())
    }
}

impl From<papyrine_journal::JournalError> for HostErr {
    fn from(e: papyrine_journal::JournalError) -> Self {
        Self::internal(e.to_string())
    }
}

impl From<serde_json::Error> for HostErr {
    fn from(e: serde_json::Error) -> Self {
        Self::internal(format!("json: {e}"))
    }
}

impl From<papyrine_writer::Error> for HostErr {
    fn from(e: papyrine_writer::Error) -> Self {
        Self::new(Code::Io, e.to_string())
    }
}

/// True when the error means the child process is gone (as opposed to a
/// request the child rejected).
pub fn is_crash(e: &IpcError) -> bool {
    e.code == ErrorCode::ChildCrashed
}
