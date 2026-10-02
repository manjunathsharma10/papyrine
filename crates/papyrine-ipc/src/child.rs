//! Child-side startup: find the role, take the inherited channel, apply the
//! sandbox. Everything the child needs from the host arrives afterwards over
//! the channel; it never opens the user's files itself.

use crate::proto::Role;
use crate::spawn::{ENV_ROLE, ENV_SANDBOX};
use crate::transport::Endpoint;
use papyrine_sandbox::{Profile, Report};

pub struct Bootstrap {
    pub role: Role,
    pub endpoint: Endpoint,
    /// What the sandbox enforced (`None` when the host started us unsandboxed).
    pub sandbox: Option<Report>,
    pub profile: Option<Profile>,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("bad role {0:?}")]
    BadRole(String),
    #[error("channel: {0}")]
    Channel(#[from] std::io::Error),
    #[error("bad sandbox profile: {0}")]
    Profile(String),
    #[error("sandbox: {0}")]
    Sandbox(#[from] papyrine_sandbox::Error),
}

/// `Ok(None)` when this process was not started as a child.
///
/// The sandbox is applied *here*, before any thread is started by the caller
/// and before the child touches PDFium/qpdf data. Libraries the child needs
/// must either be loaded afterwards from an allowlisted directory or loaded
/// before calling this.
pub fn bootstrap() -> Result<Option<Bootstrap>, BootstrapError> {
    let Ok(role) = std::env::var(ENV_ROLE) else {
        return Ok(None);
    };
    let role = Role::from_arg(&role).ok_or(BootstrapError::BadRole(role))?;
    let endpoint = crate::transport::endpoint_from_env()?;
    let profile = match std::env::var(ENV_SANDBOX) {
        Ok(json) => Some(
            serde_json::from_str::<Profile>(&json)
                .map_err(|e| BootstrapError::Profile(e.to_string()))?,
        ),
        Err(_) => None,
    };
    let sandbox = match &profile {
        Some(p) => Some(papyrine_sandbox::apply_child_sandbox(p)?),
        None => None,
    };
    Ok(Some(Bootstrap {
        role,
        endpoint,
        sandbox,
        profile,
    }))
}
