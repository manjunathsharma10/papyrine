//! papyrine-annotate: annotation commands for Papyrine (ROADMAP 1.12, ADR-007).
//!
//! Commands (see [`commands`]) create, modify, erase and delete the MVP annotation types:
//! highlight, underline, strike-out, squiggly, sticky note, text box (FreeText), pen ink,
//! rectangle, oval, line and arrow, plus replies. Each writes a complete appearance stream
//! (see [`appearance`]); text boxes are typeset with rustybuzz and embed font subsets as
//! Type0/CIDFontType2 with a ToUnicode map (see [`text`]). Annotations of other types are
//! never touched except by an explicit metadata edit or delete.
//!
//! Register with [`register`]; every command rebuilds from its `params()` JSON.

pub mod appearance;
pub mod catalog;
pub mod commands;
mod erase;
mod geometry;
mod objects;
mod props;
mod spec;
pub mod text;

pub use commands::{
    AddAnnotation, AddReply, AnnotRef, DeleteAnnotations, EraseInk, UpdateAnnotation, register,
};
pub use erase::erase_strokes;
pub use geometry::*;
pub use objects::{parse_da, read_spec};
pub use props::*;
pub use spec::*;
pub use text::{FontReport, FontUse};
