use crate::repair::RepairLog;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Typed failures. C++ exceptions are converted in the shim and never unwind into Rust.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The document is encrypted and the password is missing or wrong.
    #[error("invalid or missing password")]
    InvalidPassword,
    /// The file is damaged beyond what recovery could repair.
    #[error("damaged PDF: {message}")]
    Damaged { message: String, repairs: RepairLog },
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// File system error reported by qpdf (open/read/write).
    #[error("I/O error: {0}")]
    Io(String),
    #[error("page tree error: {0}")]
    Pages(String),
    #[error("object error: {0}")]
    Object(String),
    /// An accessor was used on an object of the wrong type.
    #[error("{0}")]
    Type(String),
    #[error("{0}")]
    Range(String),
    /// qpdf reported an internal error (a bug or an unexpected state).
    #[error("qpdf internal error: {0}")]
    Internal(String),
    /// A C++ exception that is not a qpdf error (bad_alloc, std::exception, ...).
    #[error("native error: {0}")]
    Native(String),
}

impl Error {
    pub(crate) fn from_exception(e: cxx::Exception) -> Error {
        Self::from_message(e.what())
    }

    pub(crate) fn from_message(raw: &str) -> Error {
        let (code, msg) = match raw.split_once('|') {
            Some((c, m)) => (c.parse::<i32>().unwrap_or(100), m.to_string()),
            None => (100, raw.to_string()),
        };
        match code {
            1 => Error::Internal(msg),
            2 => Error::Io(msg),
            3 => Error::Unsupported(msg),
            4 => Error::InvalidPassword,
            5 => Error::Damaged {
                message: msg,
                repairs: RepairLog::default(),
            },
            6 => Error::Pages(msg),
            7 => Error::Object(msg),
            110 => Error::Type(msg),
            111 => Error::Range(msg),
            102 => Error::Native("out of memory".into()),
            _ => Error::Native(msg),
        }
    }

    pub(crate) fn with_repairs(self, log: RepairLog) -> Error {
        match self {
            Error::Damaged { message, .. } => Error::Damaged {
                message,
                repairs: log,
            },
            other => other,
        }
    }
}

impl From<cxx::Exception> for Error {
    fn from(e: cxx::Exception) -> Self {
        Error::from_exception(e)
    }
}
