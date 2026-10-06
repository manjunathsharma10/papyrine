use papyrine_forms::Alert;

pub type Result<T, E = FillError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum FillError {
    #[error(transparent)]
    Cos(#[from] papyrine_cos::Error),
    #[error(transparent)]
    Ops(#[from] papyrine_ops::Error),
    #[error("no such field: {0}")]
    NoSuchField(String),
    #[error("field `{0}` is read-only")]
    ReadOnly(String),
    #[error("field `{field}`: {what}")]
    Invalid { field: String, what: String },
    /// The document has dynamic XFA; the AcroForm is only a placeholder and is not editable.
    #[error("this form uses dynamic XFA and is read-only in Papyrine")]
    DynamicXfa,
    /// A keystroke or validate script refused the value (the alerts say why).
    #[error("value rejected: {}", .0.first().map(Alert::message).unwrap_or_default())]
    Rejected(Vec<Alert>),
    /// The font cannot show a character (or no usable font exists).
    #[error("cannot generate an appearance: {0}")]
    Appearance(String),
    #[error("{0}")]
    Other(String),
}

impl FillError {
    pub fn invalid(field: &str, what: impl Into<String>) -> Self {
        FillError::Invalid {
            field: field.to_string(),
            what: what.into(),
        }
    }
}

/// Commands report `papyrine_ops::Error`; our richer error is flattened into its message.
impl From<FillError> for papyrine_ops::Error {
    fn from(e: FillError) -> Self {
        match e {
            FillError::Cos(c) => papyrine_ops::Error::Cos(c),
            FillError::Ops(o) => o,
            other => papyrine_ops::Error::invalid(other.to_string()),
        }
    }
}
