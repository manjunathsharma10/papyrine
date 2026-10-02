use std::{fmt, io};

/// Everything the journal can fail with.
#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    Json(serde_json::Error),
    /// Structural damage that is not a recoverable torn tail (bad magic, missing base.json...).
    Corrupt(String),
    /// The command has crashed the engine twice for this document; refuse to run it again.
    Quarantined {
        command: String,
        crashes: u32,
    },
    /// An earlier Intent never finished; the host must `resolve_unfinished` it first.
    Unresolved {
        seq: u64,
    },
    /// Another command is already in flight (the host actor serialises commands).
    Busy,
    /// `commit`/`fail` was called for a seq that is not the one in flight.
    NotInFlight,
    /// The journal was closed or discarded.
    Closed,
    /// An earlier write or fsync failed; the journal refuses further records.
    Broken(String),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "journal i/o error: {e}"),
            Self::Json(e) => write!(f, "journal json error: {e}"),
            Self::Corrupt(m) => write!(f, "journal corrupt: {m}"),
            Self::Quarantined { command, crashes } => {
                write!(
                    f,
                    "command {command:?} is quarantined after {crashes} crashes"
                )
            }
            Self::Unresolved { seq } => write!(f, "intent {seq} did not finish and is unresolved"),
            Self::Busy => write!(f, "another command is in flight"),
            Self::NotInFlight => write!(f, "that command is not in flight"),
            Self::Closed => write!(f, "journal is closed"),
            Self::Broken(m) => write!(f, "journal is broken: {m}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<io::Error> for JournalError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for JournalError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type Result<T> = std::result::Result<T, JournalError>;
