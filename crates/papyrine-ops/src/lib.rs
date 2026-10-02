//! Undoable edit commands over a `papyrine_cos::Document` (ARCHITECTURE section 4.4).
//!
//! * [`Command`]: `describe` / `apply` / `revert` / `params`. Commands mutate only through an
//!   [`EditContext`], which captures a before-image of each object at first touch and detects
//!   created objects by the maximum object number.
//! * [`ChangeSet`]: before-images, after-images, created ids, affected pages, repaint hint.
//! * [`History`]: undo/redo within a memory budget, spilling old entries to a [`SpillStore`].
//! * [`CompositeCommand`] groups commands into one undo step; [`CommandRegistry`] rebuilds
//!   commands from `params()` for replay and macros.
//! * [`verify::Shadow`]: fingerprints every object before and after a command and fails if
//!   something changed that the `ChangeSet` does not account for. `History` runs it in debug
//!   builds and tests.
//!
//! Undo restores objects in place from their before-images; objects created by an undone
//! command stay in memory as unreachable orphans so redo can restore exact object numbers.

mod changeset;
mod command;
pub mod commands;
mod context;
mod error;
mod history;
mod image;
pub mod pagetree;
mod registry;
mod store;
mod text;
pub mod verify;

pub use changeset::{ChangeSet, ChangeSummary, RepaintHint};
pub use command::{Command, CompositeCommand};
pub use commands::{
    BoxKind, DeletePages, DuplicatePages, InfoField, InsertBlankPage, MovePages, RotatePages,
    SetInfoField, SetPageBox,
};
pub use context::EditContext;
pub use error::{Error, Mismatch, Result};
pub use history::History;
pub use image::{ObjectImage, TRAILER};
pub use registry::{CommandRegistry, describe_command};
pub use store::{DirStore, MemStore, SpillStore};
pub use text::{LocalizedText, decode_text_string, encode_text_string};
