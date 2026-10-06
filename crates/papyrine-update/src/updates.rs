//! `updates.json` and the banner decision (ARCHITECTURE 9.2).

use crate::envelope::{DOMAIN_UPDATES, Envelope};
use crate::keyring::{KeyPurpose, Keyring};
use crate::{Error, Result};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Security,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    /// `macos`, `windows` or `linux` (as `std::env::consts::OS`).
    pub os: String,
    /// `aarch64`, `x86_64`, ... (as `std::env::consts::ARCH`).
    pub arch: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the installer.
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub version: String,
    pub severity: Severity,
    #[serde(default)]
    pub advisory: String,
    /// Semver requirements (`">=0.1.0, <0.1.3"`); any match means affected.
    /// Empty means every version older than `version`.
    #[serde(default)]
    pub affected: Vec<String>,
    pub downloads: Vec<Download>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Updates {
    pub schema: u32,
    pub serial: u64,
    pub issued: u64,
    /// Unix seconds after which the file is stale and ignored.
    pub expires: u64,
    pub channels: BTreeMap<String, Channel>,
}

/// What the UI should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Banner {
    /// Installed version is current (or newer).
    UpToDate,
    /// A normal release: shown in About only.
    NormalAvailable { version: String },
    /// A security release affecting the installed version: non-modal banner.
    Security {
        version: String,
        advisory: String,
        download: Option<Download>,
    },
}

impl Updates {
    /// Verify `env` against `keyring` at time `now` and parse it.
    pub fn verify(env: &Envelope, keyring: &Keyring, now: u64) -> Result<Updates> {
        // Any listed signer that the keyring authorises and that verifies wins.
        let mut last = Error::BadSignature("no signature".into());
        for s in &env.signatures {
            match keyring
                .key_for(&s.key_id, KeyPurpose::Release, now)
                .and_then(|k| env.verify_with(DOMAIN_UPDATES, &s.key_id, &k))
            {
                Ok(()) => {
                    let u: Updates = serde_json::from_str(&env.signed)
                        .map_err(|e| Error::Malformed(format!("updates: {e}")))?;
                    if u.schema != 1 {
                        return Err(Error::Malformed(format!(
                            "unsupported updates schema {}",
                            u.schema
                        )));
                    }
                    if now >= u.expires {
                        return Err(Error::Stale("updates.json has expired".into()));
                    }
                    return Ok(u);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Decide what to show for `installed` on `channel`.
    pub fn decide(&self, installed: &str, channel: &str, os: &str, arch: &str) -> Result<Banner> {
        let Some(ch) = self.channels.get(channel) else {
            return Ok(Banner::UpToDate);
        };
        let installed = parse_version(installed)?;
        let latest = parse_version(&ch.version)?;
        if latest <= installed {
            return Ok(Banner::UpToDate);
        }
        if ch.severity == Severity::Normal || !affects(ch, &installed, &latest)? {
            return Ok(Banner::NormalAvailable {
                version: ch.version.clone(),
            });
        }
        let download = ch
            .downloads
            .iter()
            .find(|d| d.os == os && d.arch == arch)
            .cloned();
        Ok(Banner::Security {
            version: ch.version.clone(),
            advisory: ch.advisory.clone(),
            download,
        })
    }
}

fn parse_version(s: &str) -> Result<Version> {
    Version::parse(s.trim_start_matches('v'))
        .map_err(|e| Error::Malformed(format!("version {s:?}: {e}")))
}

fn affects(ch: &Channel, installed: &Version, latest: &Version) -> Result<bool> {
    if ch.affected.is_empty() {
        return Ok(installed < latest);
    }
    for r in &ch.affected {
        let req = VersionReq::parse(r)
            .map_err(|e| Error::Malformed(format!("affected range {r:?}: {e}")))?;
        if req.matches(installed) {
            return Ok(true);
        }
    }
    Ok(false)
}
