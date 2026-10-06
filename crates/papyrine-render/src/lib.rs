//! papyrine-render: PDFium-backed tile renderer, previews and text layer
//! (docs/ARCHITECTURE.md §1.2, §4.6, §4.7).
//!
//! Documents are opened through `FPDF_LoadCustomDocument` over a
//! [`MultiBuf`] (base mmap ‖ appended incremental sections), so the file is
//! never copied onto the heap. PDFium is loaded dynamically, see [`Library`].

mod document;
mod error;
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
pub use source::{Bytes, MultiBuf, bytes_from_vec, open_mmap};
pub use tiles::{TILE_SIZE, TileCoord};

/// A cancel callback that never cancels.
pub fn never_cancel() -> bool {
    false
}
