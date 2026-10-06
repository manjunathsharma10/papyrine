//! papyrine-render: PDFium-backed tile renderer, previews and text layer
//! (docs/ARCHITECTURE.md §1.2, §4.6, §4.7).
//!
//! Documents are opened through `FPDF_LoadCustomDocument` over a
//! [`MultiBuf`] (base mmap ‖ appended incremental sections), so the file is
//! never copied onto the heap. PDFium is loaded dynamically, see [`Library`].
//!
//! * [`role`]: the renderer child (one PDFium thread, prioritised cancellable
//!   queue, debounced re-open, search, text cache, warm-up) over `papyrine-ipc`.
//! * [`governor`](MemoryConfig) and [`mem`]: the memory policy that keeps the
//!   renderer inside its large-document budget (docs/spikes/0.2-memory.md).
//!   On macOS the child must start with [`mem::CHILD_ENV`].
//! * [`golden`]: hash plus perceptual signature support for the golden pages.

mod document;
mod error;
pub mod golden;
mod governor;
mod library;
pub mod mem;
pub mod role;
pub mod sched;
mod source;
pub mod testgen;
pub mod tiles;

pub use document::{CharBox, Document, Glyphs, PAGE_CACHE_CAP, PageText, Tile};
pub use error::{Error, Result};
pub use governor::{ColdLoad, Governor, MemoryConfig};
pub use library::{Library, platform_dir_name};
pub use role::{RendererConfig, Stats, run_renderer, serve};
pub use source::{Bytes, MultiBuf, bytes_from_vec, open_mmap};
pub use tiles::{TILE_SIZE, TileCoord};

/// A cancel callback that never cancels.
pub fn never_cancel() -> bool {
    false
}
