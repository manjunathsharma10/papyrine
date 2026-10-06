//! The renderer role as a standalone child executable: what the multi-role
//! `papyrine` binary runs when it is started with `PAPYRINE_ROLE=renderer`.
//! Used by the large-document memory gate (`bench/memory`) and the process
//! tests; it is also the reference for wiring the role into the real binary:
//!
//! 1. `mem::reexec_with_child_env()` first, so libmalloc returns freed memory
//!    (macOS; see `papyrine_render::mem`), before any sandbox is applied;
//! 2. load PDFium before `bootstrap()` applies the sandbox;
//! 3. `bootstrap()`, then `run_renderer(endpoint)`.

use papyrine_ipc::{Role, ServeExit, bootstrap};
use papyrine_render::{Library, mem, role};

fn main() {
    if let Err(e) = mem::reexec_with_child_env() {
        eprintln!("render_role: could not re-exec with the allocator environment: {e}");
    }
    let lib = Library::global();
    let boot = match bootstrap() {
        Ok(Some(b)) if b.role == Role::Renderer => b,
        Ok(_) => {
            eprintln!("render_role: not started as a renderer child (PAPYRINE_ROLE=renderer)");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("render_role: bootstrap failed: {e}");
            std::process::exit(3);
        }
    };
    if let Err(e) = lib {
        eprintln!("render_role: {e}");
    }
    match role::run_renderer(boot.endpoint) {
        Ok(ServeExit::Shutdown | ServeExit::HostGone) => {}
        Err(e) => {
            eprintln!("render_role: {e}");
            std::process::exit(1);
        }
    }
}
