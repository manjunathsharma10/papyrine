//! Recovery offers shown at launch (ARCHITECTURE §7).

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedInfo {
    pub command: String,
    pub label: String,
    pub prior_crashes: u32,
}

/// One recovery directory found at launch.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// The recovery directory name; pass it to `restoreRecovery` / `discardRecovery`.
    pub id: String,
    pub name: String,
    pub original_path: String,
    pub saved_at_ms: u64,
    /// Committed edits that can be replayed.
    pub edits: usize,
    /// "restore" | "recovered-copy" | "refused"
    pub mode: String,
    /// Why recovery is refused (or limited), in user words.
    pub explanation: Option<String>,
    pub unfinished: Option<UnfinishedInfo>,
}

pub const MODE_RESTORE: &str = "restore";
pub const MODE_COPY: &str = "recovered-copy";
pub const MODE_REFUSED: &str = "refused";
