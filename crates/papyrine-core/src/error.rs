//! Error type shared across crates. Kinds are coarse and stable; the message
//! carries the detail.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    Io,
    /// Input could not be parsed at all.
    Parse,
    /// Input parsed only after repair, or violates the spec in a way we cannot fix.
    Corrupt,
    Encrypted,
    InvalidArgument,
    NotFound,
    Unsupported,
    Cancelled,
    OutOfMemory,
    Internal,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Parse => "parse",
            Self::Corrupt => "corrupt",
            Self::Encrypted => "encrypted",
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Unsupported => "unsupported",
            Self::Cancelled => "cancelled",
            Self::OutOfMemory => "out_of_memory",
            Self::Internal => "internal",
        }
    }
}

pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == ErrorKind::Cancelled
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Error");
        d.field("kind", &self.kind).field("message", &self.message);
        if let Some(s) = &self.source {
            d.field("source", &s.to_string());
        }
        d.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        let kind = if e.kind() == std::io::ErrorKind::NotFound {
            ErrorKind::NotFound
        } else {
            ErrorKind::Io
        };
        Error::new(kind, e.to_string()).with_source(e)
    }
}

impl From<crate::cancel::Cancelled> for Error {
    fn from(_: crate::cancel::Cancelled) -> Self {
        Error::new(ErrorKind::Cancelled, "operation cancelled")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_kind() {
        let e = Error::new(ErrorKind::Parse, "bad token at 12");
        assert_eq!(e.to_string(), "parse: bad token at 12");
        assert_eq!(e.kind(), ErrorKind::Parse);
    }

    #[test]
    fn io_conversion_maps_not_found() {
        let io = std::io::Error::from(std::io::ErrorKind::NotFound);
        let e: Error = io.into();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn cancelled_conversion() {
        let e: Error = crate::cancel::Cancelled.into();
        assert!(e.is_cancelled());
    }
}
