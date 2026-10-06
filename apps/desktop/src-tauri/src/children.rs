//! Role dispatch for the single multi-role executable (ARCHITECTURE §2): the host
//! re-executes itself with `PAPYRINE_ROLE=engine|renderer` in the environment. Call
//! [`run_child_if_requested`] first thing in `main`, before any thread or window exists:
//! the sandbox is applied there.

use papyrine_ipc::Role;

/// `Some(exit code)` when this process was started as a child (and has now finished);
/// `None` for the normal host.
pub fn run_child_if_requested() -> Option<i32> {
    let b = match papyrine_ipc::bootstrap() {
        Ok(Some(b)) => b,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("papyrine: child bootstrap failed: {e}");
            return Some(2);
        }
    };
    let result = match b.role {
        Role::Engine => papyrine_engine::run_engine(b.endpoint),
        Role::Renderer => papyrine_render::role::run_renderer(b.endpoint),
        Role::Other(name) => {
            eprintln!("papyrine: no component helper named {name:?} in this build");
            return Some(2);
        }
    };
    match result {
        Ok(_) => Some(0),
        Err(e) => {
            eprintln!("papyrine: child exited: {e}");
            Some(1)
        }
    }
}

/// Where PDFium lives for children started from a development checkout. Packaged builds
/// ship it beside the executable, which the sandbox profile already allows.
pub fn dev_pdfium_dir() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("PAPYRINE_PDFIUM_DIR") {
        return Some(p.into());
    }
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../third_party/cache/pdfium");
    let dir = root.join(papyrine_render::platform_dir_name());
    dir.is_dir()
        .then(|| std::fs::canonicalize(&dir).unwrap_or(dir))
}
