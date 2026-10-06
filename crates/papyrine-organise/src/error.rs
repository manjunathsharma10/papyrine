pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Cos(#[from] papyrine_cos::Error),
    #[error(transparent)]
    Ops(#[from] papyrine_ops::Error),
    #[error(transparent)]
    Writer(#[from] papyrine_writer::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    pub(crate) fn invalid(m: impl Into<String>) -> Error {
        Error::Invalid(m.into())
    }
}
