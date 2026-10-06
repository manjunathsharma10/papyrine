//! Starting sandboxed children by re-executing the current binary.

use std::path::PathBuf;
use std::time::Duration;

use papyrine_engine::{Request, Response};
use papyrine_ipc::{ChildClient, ChildSpec, IpcError, RenderRequest, RenderResponse, Role};

/// How to start a child. The defaults are the production ones: sandboxed, this executable.
#[derive(Clone, Debug)]
pub struct SpawnConfig {
    /// The executable to start; `None` re-executes the running one.
    pub exe: Option<PathBuf>,
    /// Apply the OS sandbox in the child. Only development tools turn this off.
    pub sandbox: bool,
    /// Address-space/commit cap (enforced on Windows by the Job object).
    pub memory_limit: Option<u64>,
    /// Extra directories the child may read (PDFium for the renderer, fonts ...).
    pub read_dirs: Vec<PathBuf>,
    pub handshake_timeout: Duration,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        SpawnConfig {
            exe: None,
            sandbox: true,
            memory_limit: None,
            read_dirs: Vec::new(),
            handshake_timeout: Duration::from_secs(30),
        }
    }
}

pub type EngineChild = ChildClient<Request, Response>;
pub type RenderChild = ChildClient<RenderRequest, RenderResponse>;

fn spec(cfg: &SpawnConfig, role: Role) -> Result<ChildSpec, IpcError> {
    // Informational (shows up in process listings); the environment carries the real role.
    let arg = match &role {
        Role::Engine => "--role=engine".to_string(),
        Role::Renderer => "--role=render".to_string(),
        Role::Other(n) => format!("--role=other:{n}"),
    };
    let mut s = ChildSpec::current_exe(role).map_err(|e| IpcError::internal(e.to_string()))?;
    if let Some(exe) = &cfg.exe {
        s.exe = exe.clone();
    }
    s.sandbox = cfg.sandbox;
    s.memory_limit = cfg.memory_limit;
    s.read_dirs.extend(cfg.read_dirs.iter().cloned());
    Ok(s.arg(arg))
}

/// Start the engine role and complete the handshake.
pub fn spawn_engine(cfg: &SpawnConfig) -> Result<EngineChild, IpcError> {
    ChildClient::spawn(&spec(cfg, Role::Engine)?, cfg.handshake_timeout)
}

/// Start the renderer role and complete the handshake.
pub fn spawn_renderer(cfg: &SpawnConfig) -> Result<RenderChild, IpcError> {
    ChildClient::spawn(&spec(cfg, Role::Renderer)?, cfg.handshake_timeout)
}
