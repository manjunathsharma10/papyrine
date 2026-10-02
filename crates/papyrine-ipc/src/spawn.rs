//! Host-side process spawning: creates the per-child temp directory and the
//! channel, starts the child (sandboxed by default) and connects a [`Client`].

use crate::client::Client;
use crate::proto::{IpcError, Role};
use crate::transport::Endpoint;
use papyrine_sandbox::{ChildTempDir, Profile};
use serde::{Serialize, de::DeserializeOwned};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub const ENV_ROLE: &str = "PAPYRINE_ROLE";
pub const ENV_SANDBOX: &str = "PAPYRINE_SANDBOX";
pub const ENV_IPC_FD: &str = "PAPYRINE_IPC_FD";

/// How to start a child.
#[derive(Clone, Debug)]
pub struct ChildSpec {
    pub exe: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub role: Role,
    /// Directories the child may read (app dir, PDFium dir, component dir ...).
    pub read_dirs: Vec<PathBuf>,
    pub read_files: Vec<PathBuf>,
    /// Apply the sandbox in the child. Only development tools turn this off.
    pub sandbox: bool,
    pub memory_limit: Option<u64>,
}

impl ChildSpec {
    /// Spawn the currently running executable in `role` (the single
    /// multi-role binary model: ROADMAP 1.9).
    pub fn current_exe(role: Role) -> io::Result<Self> {
        Ok(Self {
            exe: std::env::current_exe()?,
            args: vec![],
            env: vec![],
            role,
            read_dirs: vec![],
            read_files: vec![],
            sandbox: true,
            memory_limit: None,
        })
    }

    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn read_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.read_dirs.push(p.into());
        self
    }
    pub fn unsandboxed(mut self) -> Self {
        self.sandbox = false;
        self
    }
}

/// A running child. Dropping it kills the process if it is still alive and
/// removes its temp directory.
pub struct ChildProcess {
    inner: os::Proc,
    pub temp: ChildTempDir,
}

impl ChildProcess {
    pub fn id(&self) -> u32 {
        self.inner.id()
    }
    /// Exit code (`None` if killed by a signal) once the child has exited.
    pub fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
        self.inner.try_wait()
    }
    pub fn wait(&mut self) -> io::Result<Option<i32>> {
        self.inner.wait()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        self.inner.kill()
    }
    /// Wait up to `timeout` for exit; kill if it does not exit.
    pub fn wait_or_kill(&mut self, timeout: Duration) -> io::Result<Option<i32>> {
        let end = std::time::Instant::now() + timeout;
        loop {
            if let Some(code) = self.try_wait()? {
                return Ok(code);
            }
            if std::time::Instant::now() >= end {
                let _ = self.kill();
                return self.wait();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if matches!(self.inner.try_wait(), Ok(None)) {
            let _ = self.inner.kill();
            let _ = self.inner.wait();
        }
    }
}

pub struct Spawned {
    pub process: ChildProcess,
    pub endpoint: Endpoint,
}

/// Start the child and return the host end of the channel (no handshake yet).
pub fn spawn_child(spec: &ChildSpec) -> io::Result<Spawned> {
    let temp = ChildTempDir::create()?;
    let mut profile = Profile::new(temp.path());
    profile.read_dirs = spec.read_dirs.clone();
    profile.read_files = spec.read_files.clone();
    profile.memory_limit = spec.memory_limit;
    if let Some(d) = spec.exe.parent() {
        profile.read_dirs.push(d.to_path_buf());
    }
    let profile = profile.normalized().map_err(io::Error::other)?;

    let mut env: Vec<(OsString, OsString)> = spec.env.clone();
    env.push((ENV_ROLE.into(), spec.role.as_arg().into()));
    for (k, v) in papyrine_sandbox::temp_env(&profile.temp_dir) {
        env.push((k.into(), v.into_os_string()));
    }
    if spec.sandbox {
        let json = serde_json::to_string(&profile).map_err(io::Error::other)?;
        env.push((ENV_SANDBOX.into(), json.into()));
    }
    let (inner, endpoint) = os::spawn(spec, &profile, env)?;
    Ok(Spawned {
        process: ChildProcess { inner, temp },
        endpoint,
    })
}

/// A child process plus its connected client.
pub struct ChildClient<Q, R> {
    pub client: Client<Q, R>,
    pub process: ChildProcess,
}

impl<Q, R> ChildClient<Q, R>
where
    Q: Serialize + Send + 'static,
    R: DeserializeOwned + Send + 'static,
{
    /// Spawn, handshake and return the connected pair.
    pub fn spawn(spec: &ChildSpec, handshake_timeout: Duration) -> Result<Self, IpcError> {
        let Spawned { process, endpoint } =
            spawn_child(spec).map_err(|e| IpcError::internal(format!("spawn: {e}")))?;
        let client = Client::connect(endpoint, &spec.role, handshake_timeout)?;
        Ok(Self { client, process })
    }

    /// Ask for a clean exit; kill after `timeout`. Returns the exit code.
    pub fn shutdown(mut self, timeout: Duration) -> io::Result<Option<i32>> {
        self.client.shutdown();
        self.process.wait_or_kill(timeout)
    }
}

#[cfg(unix)]
mod os {
    use super::*;
    use crate::transport::{endpoint_from_fd, socketpair};
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub struct Proc(Child);

    impl Proc {
        pub fn id(&self) -> u32 {
            self.0.id()
        }
        pub fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
            Ok(self.0.try_wait()?.map(|s| s.code()))
        }
        pub fn wait(&mut self) -> io::Result<Option<i32>> {
            Ok(self.0.wait()?.code())
        }
        pub fn kill(&mut self) -> io::Result<()> {
            self.0.kill()
        }
    }

    const CHILD_FD: i32 = 3;

    pub fn spawn(
        spec: &ChildSpec,
        _profile: &Profile,
        env: Vec<(OsString, OsString)>,
    ) -> io::Result<(Proc, Endpoint)> {
        let (parent, child) = socketpair()?;
        let mut cmd = Command::new(&spec.exe);
        cmd.args(&spec.args);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.env(ENV_IPC_FD, CHILD_FD.to_string());
        let child_raw = child.as_raw_fd();
        // SAFETY: only async-signal-safe calls (dup2, fcntl) between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if child_raw == CHILD_FD {
                    let fl = libc::fcntl(CHILD_FD, libc::F_GETFD);
                    if fl < 0 || libc::fcntl(CHILD_FD, libc::F_SETFD, fl & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                } else if libc::dup2(child_raw, CHILD_FD) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let proc = cmd.spawn()?;
        drop(child); // the child has its own copy
        Ok((Proc(proc), endpoint_from_fd(parent)?))
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use crate::transport::windows::{Peer, endpoint_from_pipes, pipe, set_inheritable};
    use papyrine_sandbox::windows::{RestrictedChild, SpawnRequest, spawn_restricted};
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};

    pub enum Proc {
        Plain(Child),
        Restricted(RestrictedChild),
    }

    impl Proc {
        pub fn id(&self) -> u32 {
            match self {
                Proc::Plain(c) => c.id(),
                Proc::Restricted(c) => c.id(),
            }
        }
        pub fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
            match self {
                Proc::Plain(c) => Ok(c.try_wait()?.map(|s| s.code())),
                Proc::Restricted(c) => Ok(c.try_wait()?.map(|code| Some(code as i32))),
            }
        }
        pub fn wait(&mut self) -> io::Result<Option<i32>> {
            match self {
                Proc::Plain(c) => Ok(c.wait()?.code()),
                Proc::Restricted(c) => Ok(Some(c.wait()? as i32)),
            }
        }
        pub fn kill(&mut self) -> io::Result<()> {
            match self {
                Proc::Plain(c) => c.kill(),
                Proc::Restricted(c) => c.kill(),
            }
        }
    }

    pub fn spawn(
        spec: &ChildSpec,
        profile: &Profile,
        mut env: Vec<(OsString, OsString)>,
    ) -> io::Result<(Proc, Endpoint)> {
        let (to_child_r, to_child_w) = pipe()?; // host writes, child reads
        let (from_child_r, from_child_w) = pipe()?; // child writes, host reads
        set_inheritable(&to_child_r, true)?;
        set_inheritable(&from_child_w, true)?;
        env.push((
            "PAPYRINE_IPC_RX".into(),
            (to_child_r.as_raw_handle() as usize).to_string().into(),
        ));
        env.push((
            "PAPYRINE_IPC_TX".into(),
            (from_child_w.as_raw_handle() as usize).to_string().into(),
        ));

        let (proc, peer) = if spec.sandbox {
            let inherit = [
                to_child_r.as_raw_handle() as _,
                from_child_w.as_raw_handle() as _,
            ];
            let child = spawn_restricted(&SpawnRequest {
                exe: &spec.exe,
                args: &spec.args,
                env: &env,
                inherit: &inherit,
                profile,
            })?;
            let peer = Peer::process(child.duplicate_process_handle()?);
            (Proc::Restricted(child), peer)
        } else {
            let mut cmd = Command::new(&spec.exe);
            cmd.args(&spec.args);
            for (k, v) in &env {
                cmd.env(k, v);
            }
            let child = cmd.spawn()?;
            // PROCESS_DUP_HANDLE access for sending handles: reopen by pid.
            let peer = open_process_for_dup(child.id())?;
            (Proc::Plain(child), peer)
        };
        // The child owns its ends now; close ours.
        drop(to_child_r);
        drop(from_child_w);
        Ok((proc, endpoint_from_pipes(to_child_w, from_child_r, peer)))
    }

    fn open_process_for_dup(pid: u32) -> io::Result<Peer> {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_DUP_HANDLE};
        // SAFETY: plain OpenProcess call.
        let h = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh handle we own.
        Ok(Peer::process(unsafe {
            OwnedHandle::from_raw_handle(h as _)
        }))
    }
}
