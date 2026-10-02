use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Cos(#[from] papyrine_cos::Error),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Something the writer deliberately refuses to emit rather than risk corrupting the file.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The existing file's cross-reference chain cannot be extended by an incremental update.
    #[error("cannot append to this file: {0}")]
    Chain(String),
    /// The temp file failed validation; the target was not touched.
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    pub(crate) fn unsupported(msg: impl Into<String>) -> Error {
        Error::Unsupported(msg.into())
    }
}
