use papyrine_cos::ObjId;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the shadow verifier found wrong with a command's `ChangeSet`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mismatch {
    /// Objects whose fingerprint changed but that are not in the change set's before-images.
    pub unrecorded: Vec<ObjId>,
    /// Objects that appeared during the command but are not listed as created.
    pub unlisted_created: Vec<ObjId>,
    /// Objects whose recorded after-image differs from the live object.
    pub stale_after: Vec<ObjId>,
}

impl Mismatch {
    pub fn is_empty(&self) -> bool {
        self.unrecorded.is_empty()
            && self.unlisted_created.is_empty()
            && self.stale_after.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Cos(#[from] papyrine_cos::Error),
    #[error("invalid command parameters: {0}")]
    InvalidParams(String),
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("object {0} is direct; record the indirect object that holds it")]
    NotIndirect(String),
    /// Shadow verifier: a command changed objects it did not record (a bug in the command).
    #[error("command `{command}` is not fully recorded in its ChangeSet: {mismatch:?}")]
    Unrecorded { command: String, mismatch: Mismatch },
    /// Shadow verifier: an undo or redo did not reproduce the recorded images exactly.
    #[error("restore of `{command}` did not reproduce the recorded state for {ids:?}")]
    RestoreMismatch { command: String, ids: Vec<ObjId> },
    #[error("history spill store: {0}")]
    Spill(String),
    #[error("corrupt object image: {0}")]
    Corrupt(String),
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Error {
        Error::InvalidParams(msg.into())
    }
}
