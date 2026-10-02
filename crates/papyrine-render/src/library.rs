//! Locating and loading libpdfium (dynamic binding via `pdfium-render`'s loader).

use crate::error::{Error, Result};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The loaded PDFium library. One per process; calls are serialised by the
/// binding layer (`thread_safe`), so documents may live on different threads,
/// but the renderer is designed to be driven from a single thread.
pub struct Library {
    pub(crate) bindings: Box<dyn PdfiumLibraryBindings>,
    path: PathBuf,
}

static GLOBAL: OnceLock<std::result::Result<Arc<Library>, String>> = OnceLock::new();

pub fn platform_dir_name() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-arm64"
    } else if cfg!(target_os = "macos") {
        "mac-x64"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "win-arm64"
    } else if cfg!(target_os = "windows") {
        "win-x64"
    } else {
        "linux-x64"
    }
}

fn candidate_dirs(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = explicit {
        roots.push(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("PAPYRINE_PDFIUM_DIR") {
        roots.push(PathBuf::from(p));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    roots.push(
        manifest
            .join("../../third_party/cache/pdfium")
            .join(platform_dir_name()),
    );
    roots.push(Path::new("third_party/cache/pdfium").join(platform_dir_name()));
    roots
        .into_iter()
        .flat_map(|r| [r.join("lib"), r.join("bin"), r])
        .collect()
}

impl Library {
    /// Load (once) from `dir`, `$PAPYRINE_PDFIUM_DIR`, or
    /// `third_party/cache/pdfium/<platform>`; each is searched directly and in
    /// its `lib/` and `bin/` subdirectories.
    pub fn load(dir: Option<&Path>) -> Result<Arc<Library>> {
        GLOBAL
            .get_or_init(|| Self::load_uncached(dir).map(Arc::new))
            .clone()
            .map_err(Error::Library)
    }

    /// The already-loaded library or a default-location load.
    pub fn global() -> Result<Arc<Library>> {
        Self::load(None)
    }

    fn load_uncached(dir: Option<&Path>) -> std::result::Result<Library, String> {
        let name = Pdfium::pdfium_platform_library_name();
        let mut tried = Vec::new();
        for d in candidate_dirs(dir) {
            let p = d.join(&name);
            if !p.is_file() {
                tried.push(p);
                continue;
            }
            let bindings =
                Pdfium::bind_to_library(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            // SAFETY: first and only initialisation of the process-global library.
            unsafe { bindings.FPDF_InitLibrary() };
            return Ok(Library { bindings, path: p });
        }
        Err(format!(
            "libpdfium not found (run tools/fetch-pdfium or set PAPYRINE_PDFIUM_DIR); tried: {}",
            tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size_bytes(&self) -> u64 {
        std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// Contents of the `VERSION` file shipped beside `lib/` (e.g. `156.0.8076.0`).
    pub fn version(&self) -> Option<String> {
        let root = self.path.parent()?.parent()?;
        let text = std::fs::read_to_string(root.join("VERSION")).ok()?;
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                .map(str::trim)
                .map(String::from)
        };
        Some(format!(
            "{}.{}.{}.{}",
            field("MAJOR")?,
            field("MINOR")?,
            field("BUILD")?,
            field("PATCH")?
        ))
    }
}
