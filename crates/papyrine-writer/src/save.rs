//! Save policy and the three save paths (ARCHITECTURE section 4.5).
//!
//! * Default: **incremental**. The file's bytes stay an exact prefix of the result.
//! * A file qpdf had to repair on open has no usable xref chain, so Save does a full optimized
//!   rewrite (renumbering; history is rebased with the returned map). Signed files are never
//!   rewritten without asking: [`SavePlan::DecisionNeeded`].
//! * After many appended bytes or sections a one-time suggestion to optimize is raised.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use papyrine_cos::{Document, ObjId, ObjectKind, Secret, WriteOptions};

use crate::atomic::{
    FaultInjector, RendererCheck, ReplaceOptions, Validation, atomic_replace, xref_was_repaired,
};
use crate::chain::{ChainState, ReadAt, XrefKind};
use crate::section::{FullWriteOptions, SectionRequest, write_full, write_section};
use crate::{Error, Result};

/// Appended bytes beyond this fraction of the first revision trigger the suggestion.
pub const SUGGEST_APPENDED_FRACTION: f64 = 0.5;
/// More xref sections than this trigger the suggestion.
pub const SUGGEST_SECTION_COUNT: u32 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestOptimize {
    AppendedBytes { appended: u64, original: u64 },
    Sections(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairReason {
    /// qpdf reconstructed the xref table when opening.
    XrefReconstructed,
    /// The tail of the file cannot be chained onto (bad startxref, loop, junk before the header).
    UnchainableTail,
}

/// Why Save needs the user's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The file is signed and damaged: appending is impossible and rewriting breaks the
    /// signatures. Offer: rewrite anyway (signatures invalid), or save a copy.
    SignedAndDamaged(RepairReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavePlan {
    Incremental { suggest: Option<SuggestOptimize> },
    FullOptimized(RepairReason),
    DecisionNeeded(Decision),
}

/// True when the document carries a digital signature (a `/ByteRange` + `/Contents` dictionary).
pub fn has_signatures(doc: &Document) -> Result<bool> {
    for id in doc.object_ids()? {
        let o = doc.object(id)?;
        let k = o.kind()?;
        if (k == ObjectKind::Dictionary || k == ObjectKind::Stream)
            && o.dict_has("ByteRange")?
            && o.dict_has("Contents")?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Decide how Save should proceed for `doc`, opened from `base`.
pub fn plan(doc: &Document, base: &(impl ReadAt + ?Sized)) -> Result<SavePlan> {
    let chain = ChainState::scan(base);
    let reason = match (&chain, xref_was_repaired(&doc.repair_log())) {
        (_, true) => Some(RepairReason::XrefReconstructed),
        (Err(_), false) => Some(RepairReason::UnchainableTail),
        (Ok(_), false) => None,
    };
    match (reason, chain) {
        (Some(r), _) => Ok(if has_signatures(doc)? {
            SavePlan::DecisionNeeded(Decision::SignedAndDamaged(r))
        } else {
            SavePlan::FullOptimized(r)
        }),
        (None, Ok(c)) => Ok(SavePlan::Incremental {
            suggest: suggestion(&c, c.len, None),
        }),
        (None, Err(e)) => Err(e),
    }
}

/// The optimize suggestion for a file of `new_len` bytes whose chain is `chain`.
pub fn suggestion(
    chain: &ChainState,
    new_len: u64,
    baseline: Option<u64>,
) -> Option<SuggestOptimize> {
    let original = baseline.or(chain.first_revision_len)?;
    let appended = new_len.saturating_sub(original);
    if original > 0 && appended as f64 > original as f64 * SUGGEST_APPENDED_FRACTION {
        return Some(SuggestOptimize::AppendedBytes { appended, original });
    }
    (chain.sections > SUGGEST_SECTION_COUNT).then_some(SuggestOptimize::Sections(chain.sections))
}

#[derive(Default)]
pub struct SaveOptions<'a> {
    /// Password of an encrypted document (used to validate the result).
    pub password: Option<Secret>,
    /// Host callback that opens the temp file in the renderer and renders page 1.
    pub renderer: Option<RendererCheck<'a>>,
    /// Length of the original file for the 50% rule, when the host tracks it.
    pub baseline_len: Option<u64>,
    pub faults: Option<&'a mut dyn FaultInjector>,
    /// Skip the exact-prefix comparison against the base (O(file size)).
    pub skip_prefix_check: bool,
    /// Allow [`save_optimized`] to rewrite a signed document, which invalidates its signatures.
    /// Set only after the user confirmed (see [`Decision`]).
    pub break_signatures: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveKind {
    Incremental,
    FullIdPreserving,
    Optimized,
}

#[derive(Debug, Clone)]
pub struct SaveReport {
    pub kind: SaveKind,
    pub bytes_written: u64,
    /// Bytes added after the base (incremental only).
    pub appended: u64,
    pub sections: u32,
    pub suggest: Option<SuggestOptimize>,
    /// Old to new object numbers (optimized rewrite only): rebase undo history through it.
    pub renumbering: Option<HashMap<ObjId, ObjId>>,
    pub dir_synced: bool,
}

#[derive(Debug)]
pub enum SaveOutcome {
    Saved(SaveReport),
    DecisionNeeded(Decision),
}

fn page_count(doc: &Document) -> Option<usize> {
    doc.page_count().ok()
}

/// Incremental save: copy `base` to a temp file, append the section for `req`, validate, and
/// atomically replace `target` (which may be `base` itself or a different path for Save As).
pub fn save_incremental<'a>(
    doc: &Document,
    base: &'a Path,
    target: &Path,
    req: &SectionRequest,
    opts: SaveOptions<'a>,
) -> Result<SaveReport> {
    let base_file = File::open(base)?;
    let chain = ChainState::scan(&base_file)?;
    let section = write_section(doc, &chain, req)?;
    let base_len = chain.len;
    let bytes = &section.bytes;
    let outcome = atomic_replace(
        target,
        |tmp| {
            // `fs::copy` clones on APFS (only onto a path that does not exist yet) and uses
            // copy_file_range on Linux, so a save does not rewrite a large file.
            fs::remove_file(tmp)?;
            fs::copy(base, tmp)?;
            let mut f = File::options().append(true).open(tmp)?;
            if f.metadata()?.len() != base_len {
                return Err(Error::Chain("the file changed since it was scanned".into()));
            }
            f.write_all(bytes)?;
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                password: opts.password.clone(),
                expect_pages: page_count(doc),
                prefix_of: (!opts.skip_prefix_check).then_some((base, base_len)),
                allow_repairs: false,
                renderer: opts.renderer,
            },
            faults: opts.faults,
        },
    )?;
    Ok(SaveReport {
        kind: SaveKind::Incremental,
        bytes_written: section.chain.len,
        appended: section
            .chain
            .len
            .saturating_sub(opts.baseline_len.unwrap_or(base_len)),
        sections: section.chain.sections,
        suggest: suggestion(&section.chain, section.chain.len, opts.baseline_len),
        renumbering: None,
        dir_synced: outcome.dir_synced,
    })
}

/// ID-preserving full write to `target` (recovery checkpoints, render base).
pub fn save_full(
    doc: &Document,
    target: &Path,
    xref: XrefKind,
    opts: SaveOptions<'_>,
) -> Result<SaveReport> {
    let mut written = 0u64;
    let outcome = atomic_replace(
        target,
        |tmp| {
            let mut w = BufWriter::with_capacity(1 << 20, File::create(tmp)?);
            let r = write_full(
                doc,
                &mut w,
                &FullWriteOptions {
                    xref,
                    ..FullWriteOptions::default()
                },
                &|| false,
            )?;
            w.flush()?;
            written = r.bytes;
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                password: opts.password.clone(),
                expect_pages: page_count(doc),
                prefix_of: None,
                allow_repairs: false,
                renderer: opts.renderer,
            },
            faults: opts.faults,
        },
    )?;
    Ok(SaveReport {
        kind: SaveKind::FullIdPreserving,
        bytes_written: written,
        appended: 0,
        sections: 1,
        suggest: None,
        renumbering: None,
        dir_synced: outcome.dir_synced,
    })
}

/// A trailer rebuilt from a damaged file can lack `/Size`, and qpdf's writer then copies the
/// gap into its output, producing a file qpdf itself calls damaged. Give it one.
fn ensure_trailer_size(doc: &Document) -> Result<()> {
    let trailer = doc.trailer()?;
    if !trailer.dict_has("Size")? {
        let max = doc.object_ids()?.iter().map(|i| i.num).max().unwrap_or(0);
        trailer.dict_set("Size", &doc.new_int(i64::from(max) + 1))?;
    }
    Ok(())
}

/// Optimized full rewrite through qpdf's writer. Object numbers change: the returned report
/// carries the old-to-new map.
pub fn save_optimized(
    doc: &Document,
    target: &Path,
    write_opts: &WriteOptions,
    opts: SaveOptions<'_>,
) -> Result<SaveReport> {
    if !opts.break_signatures && has_signatures(doc)? {
        return Err(Error::SignedFile);
    }
    ensure_trailer_size(doc)?;
    let mut renumbering = None;
    let outcome = atomic_replace(
        target,
        |tmp| {
            let out = doc.write_to_path(tmp, write_opts)?;
            renumbering = Some(out.renumbering().clone());
            Ok(())
        },
        ReplaceOptions {
            validation: Validation {
                password: opts.password.clone(),
                expect_pages: page_count(doc),
                prefix_of: None,
                // A damaged source can legitimately make qpdf's output warn on re-read.
                allow_repairs: false,
                renderer: opts.renderer,
            },
            faults: opts.faults,
        },
    )?;
    let len = fs::metadata(target)?.len();
    Ok(SaveReport {
        kind: SaveKind::Optimized,
        bytes_written: len,
        appended: 0,
        sections: 1,
        suggest: None,
        renumbering,
        dir_synced: outcome.dir_synced,
    })
}

/// Save following the policy. Damaged unsigned files get a full optimized rewrite; damaged
/// signed files return [`SaveOutcome::DecisionNeeded`] and nothing is written.
pub fn save<'a>(
    doc: &Document,
    base: &'a Path,
    target: &Path,
    req: &SectionRequest,
    optimized_opts: &WriteOptions,
    opts: SaveOptions<'a>,
) -> Result<SaveOutcome> {
    let base_file = File::open(base)?;
    match plan(doc, &base_file)? {
        SavePlan::Incremental { .. } => {
            save_incremental(doc, base, target, req, opts).map(SaveOutcome::Saved)
        }
        SavePlan::FullOptimized(_) => {
            save_optimized(doc, target, optimized_opts, opts).map(SaveOutcome::Saved)
        }
        SavePlan::DecisionNeeded(d) => Ok(SaveOutcome::DecisionNeeded(d)),
    }
}
