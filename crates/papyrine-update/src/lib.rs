//! Opt-in security update check (ARCHITECTURE section 9.2, ADR-022/023).
//!
//! Nothing here touches the network unless the user opted in
//! ([`settings::UpdateSettings`], [`policy::Policy`]). Everything fetched is
//! authenticated by an Ed25519 chain: embedded root keys -> `keyring.json`
//! -> release key -> `updates.json` -> SHA-256 of each installer.

#[cfg(all(feature = "dev-root", not(debug_assertions)))]
compile_error!("the `dev-root` feature must not be enabled in release builds (ADR-023)");

pub mod check;
pub mod download;
pub mod envelope;
pub mod keyring;
pub mod policy;
pub mod roots;
pub mod settings;
pub mod transport;
pub mod updates;

pub use check::{CheckError, CheckOutcome, Checker, Skip, Trigger};
pub use envelope::Envelope;
pub use keyring::{KeyPurpose, Keyring, OnlineKey};
pub use policy::Policy;
pub use settings::{CheckState, UpdateSettings};
pub use transport::{Request, Response, Transport, UreqTransport};
pub use updates::{Banner, Download, Updates};

/// Errors from verification and download, all of which mean "ignore and log".
#[derive(Debug)]
pub enum Error {
    /// The envelope or payload is not valid JSON of the expected shape.
    Malformed(String),
    /// No acceptable signature.
    BadSignature(String),
    /// A key is outside its validity window.
    KeyNotValid(String),
    /// A key is revoked by a root-signed keyring.
    KeyRevoked(String),
    /// A key is not authorised for this purpose.
    WrongPurpose(String),
    /// Older serial than one already accepted (rollback), or stale file.
    Stale(String),
    /// Network or filesystem failure.
    Io(String),
    /// Hash or size mismatch on a download.
    Integrity(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (k, m) = match self {
            Error::Malformed(m) => ("malformed", m),
            Error::BadSignature(m) => ("bad signature", m),
            Error::KeyNotValid(m) => ("key not valid", m),
            Error::KeyRevoked(m) => ("key revoked", m),
            Error::WrongPurpose(m) => ("wrong key purpose", m),
            Error::Stale(m) => ("stale", m),
            Error::Io(m) => ("io", m),
            Error::Integrity(m) => ("integrity", m),
        };
        write!(f, "{k}: {m}")
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
