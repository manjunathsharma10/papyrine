//! Recent files: a small JSON list in the app-data dir (most recent first).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::api::RecentFile;
use crate::util::file_name;

pub const MAX_RECENT: usize = 10;

pub struct Recent {
    file: PathBuf,
    items: Mutex<Vec<String>>,
}

impl Recent {
    pub fn load(file: PathBuf) -> Self {
        let items = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default();
        Self {
            file,
            items: Mutex::new(items),
        }
    }

    pub fn add(&self, path: &Path) {
        let p = path.to_string_lossy().into_owned();
        let mut g = self.items.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|x| *x != p);
        g.insert(0, p);
        g.truncate(MAX_RECENT);
        let json = serde_json::to_vec(&*g).unwrap_or_default();
        drop(g);
        // Best effort: a failed write only loses the recent list.
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }

    pub fn remove(&self, path: &str) {
        let mut g = self.items.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|x| x != path);
        let json = serde_json::to_vec(&*g).unwrap_or_default();
        drop(g);
        let _ = std::fs::write(&self.file, json);
    }

    /// Entries whose file still exists.
    pub fn list(&self) -> Vec<RecentFile> {
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|p| Path::new(p).exists())
            .map(|p| RecentFile {
                name: file_name(Path::new(p)),
                path: p.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_deduplicated_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let r = Recent::load(dir.path().join("recent.json"));
        let mut files = Vec::new();
        for i in 0..12 {
            let f = dir.path().join(format!("f{i}.pdf"));
            std::fs::write(&f, b"x").unwrap();
            files.push(f);
        }
        for f in &files {
            r.add(f);
        }
        r.add(&files[5]);
        let l = r.list();
        assert_eq!(l.len(), MAX_RECENT);
        assert_eq!(l[0].name, "f5.pdf");
        assert_eq!(l.iter().filter(|x| x.name == "f5.pdf").count(), 1);
        // Persisted.
        let again = Recent::load(dir.path().join("recent.json"));
        assert_eq!(again.list()[0].name, "f5.pdf");
        std::fs::remove_file(&files[5]).unwrap();
        assert!(again.list().iter().all(|x| x.name != "f5.pdf"));
    }
}
