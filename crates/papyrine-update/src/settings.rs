//! User choice, schedule and rollback counters, stored as JSON in app data.

use crate::policy::Policy;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Enabled,
    Disabled,
}

/// Effective state after combining the stored choice with policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// First run: the UI must ask, with no pre-selected answer.
    Unanswered,
    Enabled,
    Disabled,
    /// Policy forbids the check; Preferences shows the toggle locked off.
    DisabledByPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// `None` until the user answers the first-run question. Never defaulted.
    #[serde(default)]
    pub choice: Option<Choice>,
    #[serde(default)]
    pub last_check: u64,
    /// Highest accepted serials, to refuse rollback.
    #[serde(default)]
    pub keyring_serial: u64,
    #[serde(default)]
    pub updates_serial: u64,
}

impl UpdateSettings {
    pub fn load(path: &Path) -> UpdateSettings {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        let io = |e: std::io::Error| Error::Io(e.to_string());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = serde_json::to_vec_pretty(self).map_err(|e| Error::Io(e.to_string()))?;
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn state(&self, policy: &Policy) -> CheckState {
        if policy.update_check_disabled || policy.mirror().is_err() {
            return CheckState::DisabledByPolicy;
        }
        match self.choice {
            None => CheckState::Unanswered,
            Some(Choice::Enabled) => CheckState::Enabled,
            Some(Choice::Disabled) => CheckState::Disabled,
        }
    }

    /// Record the first-run or Preferences answer.
    pub fn set_choice(&mut self, enabled: bool) {
        self.choice = Some(if enabled {
            Choice::Enabled
        } else {
            Choice::Disabled
        });
    }

    pub fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.last_check) >= CHECK_INTERVAL_SECS
    }
}
