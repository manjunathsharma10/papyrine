//! Small persisted preferences the host owns (`prefs.json` in the app-data dir).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PrefData {
    /// Show the one-time "Save optimized" suggestion.
    pub optimize_suggestion: bool,
    /// "unset" until the person answers the first-run question (ARCHITECTURE §9.2).
    pub update_check: String,
}

impl Default for PrefData {
    fn default() -> Self {
        Self {
            optimize_suggestion: true,
            update_check: "unset".into(),
        }
    }
}

pub struct Prefs {
    file: PathBuf,
    data: Mutex<PrefData>,
}

impl Prefs {
    pub fn load(file: PathBuf) -> Self {
        let data = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            file,
            data: Mutex::new(data),
        }
    }

    pub fn get(&self) -> PrefData {
        self.data.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut PrefData)) {
        let mut g = self.data.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g);
        let json = serde_json::to_vec_pretty(&*g).unwrap_or_default();
        drop(g);
        if let Some(d) = self.file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_then_persist() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.json");
        let p = Prefs::load(f.clone());
        assert!(p.get().optimize_suggestion);
        assert_eq!(p.get().update_check, "unset");
        p.update(|d| {
            d.optimize_suggestion = false;
            d.update_check = "off".into();
        });
        let q = Prefs::load(f);
        assert!(!q.get().optimize_suggestion);
        assert_eq!(q.get().update_check, "off");
    }
}
