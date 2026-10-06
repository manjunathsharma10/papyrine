//! Role dispatch for the single multi-role executable.

use std::process::ExitCode;

use papyrine_ipc::{Role, ServeExit, bootstrap};

/// If this process was started as an engine or renderer child, run that role to completion and
/// return its exit code; otherwise `None` and the caller continues as the host or the CLI.
///
/// Call this first in `main`, before any thread is started: the sandbox is applied here.
pub fn run_if_child() -> Option<ExitCode> {
    let boot = match bootstrap() {
        Ok(Some(b)) => b,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("papyrine: child startup failed: {e}");
            return Some(ExitCode::from(70));
        }
    };
    let result = match boot.role {
        Role::Engine => papyrine_engine::run_engine(boot.endpoint).map_err(|e| e.to_string()),
        Role::Renderer => {
            papyrine_render::role::run_renderer(boot.endpoint).map_err(|e| e.to_string())
        }
        Role::Other(name) => {
            eprintln!("papyrine: no component helper named {name:?} is built into this binary");
            return Some(ExitCode::from(64));
        }
    };
    Some(match result {
        Ok(ServeExit::Shutdown | ServeExit::HostGone) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("papyrine: role loop failed: {e}");
            ExitCode::from(1)
        }
    })
}

/// `main` of the `papyrine-app` binary: child roles first, then the CLI.
pub fn main_entry() -> ExitCode {
    if let Some(code) = run_if_child() {
        return code;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    crate::cli::run(&args, &mut std::io::stdout(), &mut std::io::stderr())
}

/// What the `--role=` argument asks for, for invocations without the child environment
/// (`papyrine --role=cli info FILE`, the form a `papyrine` shim passes).
pub fn strip_role_arg(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut role = None;
    let mut rest = Vec::new();
    for a in args {
        match a.strip_prefix("--role=") {
            Some(r) => role = Some(r.to_string()),
            None => rest.push(a.clone()),
        }
    }
    (role, rest)
}
