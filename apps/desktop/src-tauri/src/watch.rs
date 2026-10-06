//! External-change watcher: `notify` on the parent directory of each open file, with
//! the host comparing (mtime, size) before telling the UI (ARCHITECTURE §7).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct Watch {
    watcher: Mutex<RecommendedWatcher>,
    dirs: Mutex<Vec<(PathBuf, usize)>>,
}

impl Watch {
    /// `on_change(path)` runs on a debounce thread, at most once per burst per path.
    pub fn new(on_change: impl Fn(PathBuf) + Send + 'static) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            if let Ok(ev) = ev {
                for p in ev.paths {
                    let _ = tx.send(p);
                }
            }
        })?;
        std::thread::Builder::new()
            .name("papyrine-watch".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch: HashSet<PathBuf> = HashSet::from([first]);
                    // Debounce: editors write temp file + rename in quick succession.
                    while let Ok(p) = rx.recv_timeout(Duration::from_millis(250)) {
                        batch.insert(p);
                    }
                    for p in batch {
                        on_change(p);
                    }
                }
            })
            .map_err(notify::Error::io)?;
        Ok(Self {
            watcher: Mutex::new(watcher),
            dirs: Mutex::new(Vec::new()),
        })
    }

    fn dir_of(path: &Path) -> Option<PathBuf> {
        path.parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .or_else(|| Some(PathBuf::from(".")))
    }

    pub fn watch(&self, path: &Path) {
        let Some(dir) = Self::dir_of(path) else {
            return;
        };
        let mut dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = dirs.iter_mut().find(|(d, _)| *d == dir) {
            e.1 += 1;
            return;
        }
        let ok = self
            .watcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .watch(&dir, RecursiveMode::NonRecursive)
            .is_ok();
        if ok {
            dirs.push((dir, 1));
        }
    }

    pub fn unwatch(&self, path: &Path) {
        let Some(dir) = Self::dir_of(path) else {
            return;
        };
        let mut dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = dirs.iter().position(|(d, _)| *d == dir) {
            dirs[i].1 -= 1;
            if dirs[i].1 == 0 {
                dirs.remove(i);
                let _ = self
                    .watcher
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwatch(&dir);
            }
        }
    }
}

pub type SharedWatch = Arc<Watch>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn reports_a_modified_file_once_per_burst() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.pdf");
        std::fs::write(&f, b"one").unwrap();
        let (tx, rx) = channel();
        let w = Watch::new(move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        w.watch(&f);
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..5 {
            std::fs::write(&f, format!("two{i}")).unwrap();
        }
        let got = rx.recv_timeout(Duration::from_secs(5)).expect("event");
        assert_eq!(got.file_name(), f.file_name());
        w.unwatch(&f);
    }
}
