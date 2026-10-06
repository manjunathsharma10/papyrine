//! Child supervision (ARCHITECTURE §2): starts a sandboxed child on first use, notices
//! when it died and starts a fresh one. Callers learn the child's *generation* so they
//! know when per-child state (open documents, the tile pool) must be rebuilt.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use papyrine_ipc::{ChildClient, ChildProcess, ChildSpec, Client, IpcError, Role};
use serde::{Serialize, de::DeserializeOwned};

/// How a child gets started. The default re-executes the current binary in a role;
/// tests substitute their own argument list.
pub trait Launcher: Send + Sync + 'static {
    fn spec(&self, role: Role) -> std::io::Result<ChildSpec>;
}

/// Re-executes `exe` (default: this binary) with the role in the environment.
#[derive(Clone, Debug)]
pub struct ExeLauncher {
    pub exe: Option<std::path::PathBuf>,
    pub args: Vec<std::ffi::OsString>,
    pub env: Vec<(String, String)>,
    pub read_dirs: Vec<std::path::PathBuf>,
    pub sandbox: bool,
}

impl Default for ExeLauncher {
    fn default() -> Self {
        Self {
            exe: None,
            args: vec![],
            env: vec![],
            read_dirs: vec![],
            sandbox: true,
        }
    }
}

impl ExeLauncher {
    /// Point children at a PDFium directory (development checkouts; packaged builds ship
    /// PDFium beside the executable). The sandbox allows reading it.
    pub fn with_pdfium(mut self, dir: std::path::PathBuf) -> Self {
        self.env.push((
            "PAPYRINE_PDFIUM_DIR".into(),
            dir.to_string_lossy().into_owned(),
        ));
        self.read_dirs.push(dir);
        self
    }
}

impl Launcher for ExeLauncher {
    fn spec(&self, role: Role) -> std::io::Result<ChildSpec> {
        let mut s = ChildSpec::current_exe(role)?;
        if let Some(e) = &self.exe {
            s.exe = e.clone();
        }
        s.args = self.args.clone();
        for (k, v) in &self.env {
            s = s.env(k, v);
        }
        for d in &self.read_dirs {
            s = s.read_dir(d);
        }
        s.sandbox = self.sandbox;
        Ok(s)
    }
}

struct Live<Q, R> {
    client: Client<Q, R>,
    process: Arc<Mutex<ChildProcess>>,
    generation: u64,
}

/// One supervised child of a given role.
pub struct Supervised<Q, R> {
    role: Role,
    launcher: Arc<dyn Launcher>,
    live: Mutex<Option<Live<Q, R>>>,
    generation: AtomicU64,
    restarts: AtomicU32,
    handshake: Duration,
}

/// A usable connection plus the generation it belongs to.
pub struct Conn<Q, R> {
    pub client: Client<Q, R>,
    pub generation: u64,
}

impl<Q, R> Supervised<Q, R>
where
    Q: Serialize + Send + 'static,
    R: DeserializeOwned + Send + 'static,
{
    pub fn new(role: Role, launcher: Arc<dyn Launcher>) -> Self {
        Self {
            role,
            launcher,
            live: Mutex::new(None),
            generation: AtomicU64::new(0),
            restarts: AtomicU32::new(0),
            handshake: Duration::from_secs(30),
        }
    }

    pub fn role(&self) -> &Role {
        &self.role
    }

    /// Times a dead child was replaced (not counting the first start).
    pub fn restarts(&self) -> u32 {
        self.restarts.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|l| l.client.closed_reason().is_none())
    }

    pub fn pid(&self) -> Option<u32> {
        self.live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|l| l.process.lock().unwrap_or_else(|p| p.into_inner()).id())
    }

    /// The live connection, starting (or restarting) the child when needed.
    pub fn conn(&self) -> Result<Conn<Q, R>, IpcError> {
        let mut g = self.live.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(l) = g.as_ref()
            && l.client.closed_reason().is_none()
        {
            return Ok(Conn {
                client: l.client.clone(),
                generation: l.generation,
            });
        }
        let replacing = g.take();
        if let Some(old) = &replacing {
            // Make sure a half-dead child does not linger.
            let _ = old.process.lock().unwrap_or_else(|p| p.into_inner()).kill();
        }
        drop(replacing);
        let spec = self
            .launcher
            .spec(self.role.clone())
            .map_err(|e| IpcError::internal(format!("launcher: {e}")))?;
        let cc = ChildClient::<Q, R>::spawn(&spec, self.handshake)?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if generation > 1 {
            self.restarts.fetch_add(1, Ordering::Relaxed);
        }
        let ChildClient { client, process } = cc;
        *g = Some(Live {
            client: client.clone(),
            process: Arc::new(Mutex::new(process)),
            generation,
        });
        Ok(Conn { client, generation })
    }

    /// The child's private temp directory (where it leaves output files for the host).
    pub fn temp_dir(&self) -> Option<std::path::PathBuf> {
        self.live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|l| {
                l.process
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .temp
                    .path()
                    .to_path_buf()
            })
    }

    /// Kill the child now (crash tests, "restart engine").
    pub fn kill(&self) {
        if let Some(l) = self.live.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = l.process.lock().unwrap_or_else(|p| p.into_inner()).kill();
        }
    }

    /// Ask the child to exit cleanly (app quit), killing it after `timeout`.
    pub fn shutdown(&self, timeout: Duration) {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(l) = live {
            l.client.shutdown();
            let _ = l
                .process
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .wait_or_kill(timeout);
        }
    }

    /// Has the current child died? (`conn` would replace it.)
    pub fn is_dead(&self) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|l| l.client.closed_reason().is_some())
    }
}
