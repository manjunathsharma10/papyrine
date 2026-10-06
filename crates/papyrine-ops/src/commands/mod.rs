//! Built-in commands, one per file.

mod delete_pages;
pub(crate) mod dests;
mod duplicate_pages;
pub(crate) mod forms;
pub mod import;
mod insert_blank_page;
mod insert_pages;
mod labels;
mod move_pages;
pub mod outlines;
mod rotate_pages;
mod scrub;
pub mod selection;
mod set_info_field;
mod set_page_box;

pub use delete_pages::DeletePages;
pub use dests::{name_tree_entries, norm_string as normalize_dest_name};
pub use duplicate_pages::DuplicatePages;
pub use forms::FieldReport;
pub use import::{ImportOptions, Imported, check_source_permissions, import_pages};
pub use insert_blank_page::{InsertBlankPage, PageSize};
pub use insert_pages::{BlobSource, InsertPagesFromPdf, MemoryBlobs, blob_id};
pub use move_pages::MovePages;
pub use outlines::OutlineMode;
pub use rotate_pages::RotatePages;
pub use scrub::ScrubReport;
pub use selection::{PageSelection, Parity, parse_split_ranges};
pub use set_info_field::{InfoField, SetInfoField};
pub use set_page_box::{BoxKind, SetPageBox};

use papyrine_cos::{Document, Object};

use crate::error::{Error, Result};

/// Sorted, deduplicated, range-checked page indices.
pub(crate) fn normalize_pages(doc: &Document, pages: &[usize]) -> Result<Vec<usize>> {
    let n = doc.page_count()?;
    if pages.is_empty() {
        return Err(Error::invalid("no pages selected"));
    }
    let mut v = pages.to_vec();
    v.sort_unstable();
    v.dedup();
    if let Some(&bad) = v.last().filter(|&&p| p >= n) {
        return Err(Error::invalid(format!(
            "page {bad} out of range (document has {n})"
        )));
    }
    Ok(v)
}

/// Insert `page` so it ends up at index `pos` (0..=page_count).
pub(crate) fn insert_page_at(doc: &Document, page: &Object, pos: usize) -> Result<()> {
    let n = doc.page_count()?;
    if pos >= n {
        doc.add_page(page, false)?;
    } else {
        doc.add_page_at(page, true, &doc.page(pos)?)?;
    }
    Ok(())
}

/// PDF number text: integers without a fraction, otherwise the shortest round-trip decimal.
pub(crate) fn num(x: f64) -> String {
    format!("{x}")
}
