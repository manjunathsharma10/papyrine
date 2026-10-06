//! Enterprise policy override (ARCHITECTURE 9.2): disable the check or point
//! it at a mirror. Sources: `/etc/papyrine/policy.json`, the Windows registry
//! (`HKLM\Software\Policies\Papyrine`) and macOS managed preferences
//! (`io.github.manjunathsharma10.papyrine`).

use serde::Deserialize;
use std::path::Path;

pub const POLICY_FILE: &str = "/etc/papyrine/policy.json";
pub const MAC_DOMAIN: &str = "io.github.manjunathsharma10.papyrine";
pub const REG_KEY: &str = r"HKLM\Software\Policies\Papyrine";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Policy {
    /// `UpdateCheckDisabled`: the check never runs and Preferences is locked.
    #[serde(default, alias = "UpdateCheckDisabled")]
    pub update_check_disabled: bool,
    /// `UpdateMirrorUrl`: base URL that serves `updates.json`/`keyring.json`.
    #[serde(default, alias = "UpdateMirrorUrl")]
    pub update_mirror_url: Option<String>,
}

/// A mirror URL was configured but is not an acceptable HTTPS base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMirror;

impl Policy {
    pub fn from_json(text: &str) -> Policy {
        // A policy file we cannot parse must not silently permit network use.
        serde_json::from_str(text).unwrap_or(Policy {
            update_check_disabled: true,
            update_mirror_url: None,
        })
    }

    /// Combine two sources: disabling wins, the first mirror wins.
    pub fn merge(self, other: Policy) -> Policy {
        Policy {
            update_check_disabled: self.update_check_disabled || other.update_check_disabled,
            update_mirror_url: self.update_mirror_url.or(other.update_mirror_url),
        }
    }

    /// Load from the file and the platform store of the running OS.
    pub fn load() -> Policy {
        #[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_mut))]
        let mut p = match std::fs::read_to_string(POLICY_FILE) {
            Ok(t) => Policy::from_json(&t),
            Err(_) => Policy::default(),
        };
        #[cfg(target_os = "macos")]
        {
            p = p.merge(load_macos());
        }
        #[cfg(windows)]
        {
            p = p.merge(load_windows());
        }
        p
    }

    pub fn load_from(path: &Path) -> Policy {
        match std::fs::read_to_string(path) {
            Ok(t) => Policy::from_json(&t),
            Err(_) => Policy::default(),
        }
    }

    /// The mirror base URL if one is set and acceptable. A set-but-invalid
    /// mirror yields `Err`, and the caller must not fall back to GitHub.
    pub fn mirror(&self) -> Result<Option<String>, InvalidMirror> {
        match &self.update_mirror_url {
            None => Ok(None),
            Some(u) => validate_mirror(u).map(Some).ok_or(InvalidMirror),
        }
    }
}

/// HTTPS, no credentials, query or fragment. Trailing slash trimmed.
pub fn validate_mirror(u: &str) -> Option<String> {
    let rest = u.strip_prefix("https://")?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || u.contains(['?', '#'])
        || u.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    Some(u.trim_end_matches('/').to_string())
}

#[cfg(target_os = "macos")]
fn load_macos() -> Policy {
    let path = format!("/Library/Managed Preferences/{MAC_DOMAIN}");
    if !Path::new(&format!("{path}.plist")).exists() {
        return Policy::default();
    }
    let read = |key: &str| -> Option<String> {
        let out = std::process::Command::new("/usr/bin/defaults")
            .args(["read", &path, key])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    Policy {
        update_check_disabled: read("UpdateCheckDisabled")
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "YES")),
        update_mirror_url: read("UpdateMirrorUrl").filter(|v| !v.is_empty()),
    }
}

/// Parse `reg query` output lines such as `    UpdateCheckDisabled    REG_DWORD    0x1`.
pub fn parse_reg_query(text: &str) -> Policy {
    let mut p = Policy::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(ty)) = (it.next(), it.next()) else {
            continue;
        };
        let value: Vec<&str> = it.collect();
        let value = value.join(" ");
        match (name, ty) {
            ("UpdateCheckDisabled", "REG_DWORD") => {
                p.update_check_disabled = value != "0x0" && value != "0";
            }
            ("UpdateMirrorUrl", "REG_SZ") if !value.is_empty() => {
                p.update_mirror_url = Some(value);
            }
            _ => {}
        }
    }
    p
}

#[cfg(windows)]
fn load_windows() -> Policy {
    std::process::Command::new("reg")
        .args(["query", REG_KEY])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_reg_query(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}
