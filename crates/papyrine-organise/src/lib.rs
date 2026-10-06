//! Operations that build new documents from existing ones: **merge** N files (reorderable, with
//! per-file page selections), **extract** pages (one file, or one file per page) and **split**
//! by ranges or every N pages (ROADMAP 1.14).
//!
//! They are not undoable commands: they read their inputs and produce new [`Document`]s (in
//! memory) or files (atomically, with a validated re-open). Pages travel with their
//! annotations, form fields, links, bookmarks, named destinations and page labels through the
//! same importer the "insert pages from a PDF" command uses
//! ([`papyrine_ops::import_pages`]):
//!
//! * form-field names that collide are renamed with a `_2`, `_3`, ... suffix on the top-level
//!   field, so each file keeps its own fields and values;
//! * bookmarks are kept as one top-level entry per file ([`OutlineMode::PerFile`]) or merged
//!   into one list ([`OutlineMode::Merged`]); only bookmarks that lead to imported pages come
//!   along when pages are selected;
//! * named destinations that lead to imported pages are copied (renamed on collision) and the
//!   links that use them follow; links to pages that were not imported are dropped;
//! * page labels are carried page by page: a page keeps the label it had in its source.
//!
//! Not carried: tagged-PDF structure, optional-content settings, document JavaScript, embedded
//! files and XFA (see the crate README in the milestone report).

mod error;
mod labels;
mod merge;
mod name;
mod output;
mod split;

pub use error::{Error, Result};
pub use merge::{InfoPolicy, InputReport, MergeInput, MergeOptions, MergeResult, Progress, merge};
pub use name::{NameVars, render_name, sanitize_file_name};
pub use output::{Exists, WriteSet, write_document, write_parts};
pub use papyrine_ops::{OutlineMode, PageSelection, Parity};
pub use split::{
    ExtractOptions, Part, PartIter, extract, extract_each, parse_split_ranges, split_every,
    split_ranges,
};
