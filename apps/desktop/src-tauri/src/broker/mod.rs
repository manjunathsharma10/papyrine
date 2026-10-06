//! The broker: everything the trusted host does between the UI and its sandboxed
//! children (ARCHITECTURE §2, §5, §7). It owns the engine and renderer supervisors,
//! the open documents, the tile caches and scheduler, the journal writer, the save
//! flow, recovery and the external-change watcher. It knows nothing about Tauri;
//! `tauri_app` and the dev bridge are thin adapters over [`Broker`].

mod edit;
mod engine;
mod extras;
mod open;
mod recover;
mod render;
mod save;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use papyrine_engine::proto::{Request as EngineRequest, Response as EngineResponse};
use papyrine_ipc::{RenderRequest, RenderResponse, Role};
use papyrine_journal::{Fs, RealFs, RecoveryRoot};

use crate::api::{EventSink, HostEvent};
use crate::cache::{L1, L2};
use crate::error::{HostErr, Result};
use crate::recent::Recent;
use crate::sched::TileSched;
use crate::session::{Session, parse_doc_id};
use crate::supervisor::{Launcher, Supervised};
use crate::watch::Watch;

pub use crate::recovery::Offer;
pub use recover::Restored;

/// Native dialogs. The Tauri app implements them with the OS file dialogs; the dev
/// bridge and tests use a scripted queue.
pub trait Dialogs: Send + Sync + 'static {
    fn open(&self) -> Vec<PathBuf>;
    fn save(&self, suggested_name: &str) -> Option<PathBuf>;
    /// Pick a folder (split output).
    fn folder(&self) -> Option<PathBuf>;
}

/// Dialogs that never return a file (headless defaults).
pub struct NoDialogs;
impl Dialogs for NoDialogs {
    fn open(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn save(&self, _suggested_name: &str) -> Option<PathBuf> {
        None
    }
    fn folder(&self) -> Option<PathBuf> {
        None
    }
}

pub struct Config {
    /// Per-user app data: `recovery/`, `recent.json`.
    pub data_dir: PathBuf,
    /// Temp files for dropped bytes, rebased render snapshots, recovered copies.
    pub cache_dir: PathBuf,
    pub launcher: Arc<dyn Launcher>,
    pub sink: Arc<dyn EventSink>,
    pub dialogs: Arc<dyn Dialogs>,
    pub journal: papyrine_journal::Options,
    pub watch_files: bool,
    pub render_workers: usize,
}

impl Config {
    pub fn new(
        data_dir: PathBuf,
        cache_dir: PathBuf,
        launcher: Arc<dyn Launcher>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            data_dir,
            cache_dir,
            launcher,
            sink,
            dialogs: Arc::new(NoDialogs),
            journal: papyrine_journal::Options::default(),
            watch_files: true,
            render_workers: 2,
        }
    }
}

/// Number of 1 MiB slots in the shared tile pool.
pub(crate) const POOL_SLOTS: u32 = 6;
pub(crate) const SLOT_BYTES: u64 = 1 << 20;

pub(crate) struct PoolState {
    pub generation: u64,
    pub region: Option<Arc<papyrine_ipc::SharedRegion>>,
    pub free: Vec<u32>,
}

pub(crate) struct JobCtl {
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

pub struct Inner {
    pub(crate) cfg: Config,
    pub(crate) engine: Supervised<EngineRequest, EngineResponse>,
    pub(crate) renderer: Supervised<RenderRequest, RenderResponse>,
    pub(crate) docs: Mutex<HashMap<u64, Arc<Session>>>,
    pub(crate) next_doc: AtomicU64,
    pub(crate) l1: Mutex<L1>,
    pub(crate) l2: Mutex<L2>,
    pub(crate) pool: Mutex<PoolState>,
    pub(crate) pool_cv: std::sync::Condvar,
    pub(crate) sched: OnceLock<TileSched>,
    pub(crate) recent: Recent,
    pub(crate) fs: Arc<dyn Fs>,
    pub(crate) recovery: RecoveryRoot,
    pub(crate) jobs: Mutex<HashMap<u64, JobCtl>>,
    pub(crate) next_job: AtomicU64,
    pub(crate) watch: OnceLock<Option<Watch>>,
    pub(crate) offers: Mutex<Vec<Offer>>,
    pub(crate) temp_ids: AtomicU64,
    pub(crate) pending_opens: Mutex<Option<Vec<PathBuf>>>,
    pub(crate) prompts: Mutex<HashMap<String, extras::Prompt>>,
    pub(crate) prefs: crate::prefs::Prefs,
    pub(crate) shutting_down: std::sync::atomic::AtomicBool,
}

/// Cheap to clone; all state is shared.
#[derive(Clone)]
pub struct Broker {
    pub(crate) inner: Arc<Inner>,
}

impl Broker {
    pub fn new(cfg: Config) -> Self {
        let fs: Arc<dyn Fs> = Arc::new(RealFs);
        let recovery = RecoveryRoot::new(fs.clone(), cfg.data_dir.join("recovery"));
        let recent = Recent::load(cfg.data_dir.join("recent.json"));
        let prefs = crate::prefs::Prefs::load(cfg.data_dir.join("prefs.json"));
        let engine = Supervised::new(Role::Engine, cfg.launcher.clone());
        let renderer = Supervised::new(Role::Renderer, cfg.launcher.clone());
        let workers = cfg.render_workers;
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let w = weak.clone();
            let sched = OnceLock::new();
            let _ = sched.set(TileSched::new(
                workers,
                Arc::new(move |key, cancel| match w.upgrade() {
                    Some(inner) => Broker { inner }.render_tile_now(key, cancel),
                    None => Err(HostErr::cancelled()),
                }),
            ));
            Inner {
                cfg,
                engine,
                renderer,
                docs: Mutex::new(HashMap::new()),
                next_doc: AtomicU64::new(1),
                l1: Mutex::new(L1::default()),
                l2: Mutex::new(L2::default()),
                pool: Mutex::new(PoolState {
                    generation: 0,
                    region: None,
                    free: (0..POOL_SLOTS).collect(),
                }),
                pool_cv: std::sync::Condvar::new(),
                sched,
                recent,
                fs,
                recovery,
                jobs: Mutex::new(HashMap::new()),
                next_job: AtomicU64::new(1),
                watch: OnceLock::new(),
                offers: Mutex::new(Vec::new()),
                temp_ids: AtomicU64::new(1),
                pending_opens: Mutex::new(Some(Vec::new())),
                prompts: Mutex::new(HashMap::new()),
                prefs,
                shutting_down: std::sync::atomic::AtomicBool::new(false),
            }
        });
        Broker { inner }
    }

    pub fn config(&self) -> &Config {
        &self.inner.cfg
    }

    pub(crate) fn emit(&self, e: HostEvent) {
        self.inner.cfg.sink.emit(e);
    }

    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.inner.cfg.sink
    }

    pub fn dialogs(&self) -> &Arc<dyn Dialogs> {
        &self.inner.cfg.dialogs
    }

    /// Start the renderer now (it is on the first-page path); the engine follows on
    /// another thread so it never delays first paint (ARCHITECTURE §2 "lazy start").
    pub fn warm_up(&self) {
        let b = self.clone();
        let _ = std::thread::Builder::new()
            .name("papyrine-warm-renderer".into())
            .spawn(move || {
                let _span = papyrine_core::startup::span("subsystem.renderer");
                let _ = b.inner.renderer.conn();
            });
        let b = self.clone();
        let _ = std::thread::Builder::new()
            .name("papyrine-warm-engine".into())
            .spawn(move || {
                let _span = papyrine_core::startup::span("subsystem.engine");
                let _ = b.inner.engine.conn();
            });
    }

    /// Work that must not run before the first paint: the recovery scan, the file
    /// watcher, pruning of old recovery directories, the tile-cache trim timer.
    pub fn after_first_paint(&self) {
        let b = self.clone();
        let _ = std::thread::Builder::new()
            .name("papyrine-after-paint".into())
            .spawn(move || {
                {
                    let _s = papyrine_core::startup::span("subsystem.watcher");
                    b.init_watcher();
                }
                {
                    let _s = papyrine_core::startup::span("subsystem.recovery");
                    b.scan_recovery();
                }
                b.spawn_housekeeping();
            });
    }

    fn init_watcher(&self) {
        let weak = Arc::downgrade(&self.inner);
        let w = if self.inner.cfg.watch_files {
            Watch::new(move |path| {
                if let Some(inner) = weak.upgrade() {
                    Broker { inner }.on_fs_event(&path);
                }
            })
            .ok()
        } else {
            None
        };
        let _ = self.inner.watch.set(w);
        // Documents opened before the watcher existed.
        let docs: Vec<Arc<Session>> = self.sessions();
        for s in docs {
            self.watch_session(&s);
        }
    }

    fn spawn_housekeeping(&self) {
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new()
            .name("papyrine-housekeeping".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(inner) = weak.upgrade() else { return };
                    if inner.shutting_down.load(Ordering::Relaxed) {
                        return;
                    }
                    inner
                        .l1
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .trim_if_idle(std::time::Instant::now());
                    // Journal fsyncs run on each journal's own flusher thread.
                }
            });
    }

    pub(crate) fn sessions(&self) -> Vec<Arc<Session>> {
        self.inner
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn session(&self, doc_id: &str) -> Result<Arc<Session>> {
        let id = parse_doc_id(doc_id)
            .ok_or_else(|| HostErr::not_found(format!("no such document: {doc_id}")))?;
        self.inner
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
            .ok_or_else(|| HostErr::not_found(format!("no such document: {doc_id}")))
    }

    pub(crate) fn session_by_id(&self, id: u64) -> Option<Arc<Session>> {
        self.inner
            .docs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
    }

    pub fn recent_files(&self) -> Vec<crate::api::RecentFile> {
        self.inner.recent.list()
    }

    pub fn show_open_dialog(&self) -> Vec<PathBuf> {
        self.inner.cfg.dialogs.open()
    }

    pub fn show_save_dialog(&self, suggested: &str) -> Option<PathBuf> {
        self.inner.cfg.dialogs.save(suggested)
    }

    /// Children's process ids and restart counts, for diagnostics and the crash tests.
    pub fn stats(&self) -> serde_json::Value {
        let (rendered, dropped) = self.inner.sched().counters();
        let l1 = self.inner.l1.lock().unwrap_or_else(|p| p.into_inner());
        let l2 = self.inner.l2.lock().unwrap_or_else(|p| p.into_inner());
        serde_json::json!({
            "enginePid": self.inner.engine.pid(),
            "rendererPid": self.inner.renderer.pid(),
            "engineRestarts": self.inner.engine.restarts(),
            "rendererRestarts": self.inner.renderer.restarts(),
            "openDocuments": self.inner.docs.lock().unwrap_or_else(|p| p.into_inner()).len(),
            "l1Bytes": l1.bytes(),
            "l1Tiles": l1.len(),
            "l2Bytes": l2.bytes(),
            "l2Previews": l2.len(),
            "tilesRendered": rendered,
            "tilesDroppedBeforeStart": dropped,
            "subsystemsBeforeFirstPaint": papyrine_core::startup::global().subsystems_before_first_paint(),
        })
    }

    /// Kill a child (dev bridge and crash tests).
    pub fn kill_engine(&self) {
        self.inner.engine.kill();
    }
    pub fn kill_renderer(&self) {
        self.inner.renderer.kill();
    }

    /// Orderly quit: journals of unsaved documents stay on disk (that is the point);
    /// clean documents drop theirs. Children are asked to exit.
    pub fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Relaxed);
        for s in self.sessions() {
            let _g = s.lock_cmd();
            let mut st = s.st();
            if let Some(j) = st.journal.take() {
                if st.dirty {
                    let _ = j.journal.close();
                } else {
                    let _ = j.journal.discard();
                }
            }
            for f in st.owned_files.drain(..) {
                let _ = std::fs::remove_file(f);
            }
        }
        self.inner.sched().shutdown();
        self.inner.engine.shutdown(Duration::from_secs(2));
        self.inner.renderer.shutdown(Duration::from_secs(2));
    }
}

impl Inner {
    pub(crate) fn sched(&self) -> &TileSched {
        self.sched
            .get()
            .expect("scheduler is created with the broker")
    }
}
