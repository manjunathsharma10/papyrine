//! papyrine-writer: the three outputs of the single in-memory object model (ARCHITECTURE 4.5).
//!
//! * [`write_section`]: an incremental section serialized from `papyrine_cos` handles. Used for
//!   Save, render snapshot sections and journal checkpoints. Preserves object numbers, writes a
//!   table or a stream xref to match the file, encrypts with per-object keys for encrypted files.
//! * [`write_full`]: ID-preserving full write (snapshot compaction, recovery checkpoints).
//! * [`save_optimized`]: qpdf's `QPDFWriter` rewrite; object numbers change and the old-to-new
//!   map is returned for history rebasing.
//! * [`atomic_replace`]: temp file, fsync, validation, metadata carry-over, rename, directory
//!   fsync, with fault injection for tests.
//! * [`plan`] / [`save`]: the save policy.

mod atomic;
mod chain;
mod crypto;
mod error;
mod fsutil;
mod save;
mod section;
mod syntax;

pub use atomic::{
    FaultInjector, RendererCheck, ReplaceOptions, ReplaceOutcome, Step, Validation, atomic_replace,
    cleanup_stale_temps, validate_file, xref_was_repaired,
};
pub use chain::{ChainState, ReadAt, XrefKind};
pub use crypto::{Crypto, Method};
pub use error::{Error, Result};
pub use save::{
    Decision, RepairReason, SUGGEST_APPENDED_FRACTION, SUGGEST_SECTION_COUNT, SaveKind,
    SaveOptions, SaveOutcome, SavePlan, SaveReport, SuggestOptimize, has_signatures, plan, save,
    save_full, save_incremental, save_optimized, suggestion,
};
pub use section::{
    FullWriteOptions, FullWriteReport, Section, SectionRequest, write_full, write_section,
};
pub use syntax::{format_real, write_name, write_string};
