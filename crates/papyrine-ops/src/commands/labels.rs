//! Page labels follow their pages when pages are inserted or deleted: a label range keeps
//! starting at the same page of the document's content, so ranges after the change shift.

use papyrine_cos::{ObjectKind, PageLabelRange, PageLabels};

use crate::context::EditContext;
use crate::error::Result;

fn replace(cx: &mut EditContext<'_>, ranges: Vec<PageLabelRange>) -> Result<()> {
    let doc = cx.doc();
    let root = doc.root()?;
    cx.touch(&root)?;
    doc.set_page_labels(&PageLabels { ranges })?;
    Ok(())
}

fn has_labels(cx: &EditContext<'_>) -> Result<bool> {
    Ok(cx.doc().root()?.dict_get("PageLabels")?.kind()? == ObjectKind::Dictionary)
}

/// `count` pages were inserted so the first of them has index `at` (post-insert numbering). The
/// new pages join the range that precedes them (or the first range when inserted at the front).
/// Call before the insert is applied to the labels' numbering, i.e. with the old ranges.
pub(crate) fn shift_for_insert(cx: &mut EditContext<'_>, at: usize, count: usize) -> Result<()> {
    if count == 0 || !has_labels(cx)? {
        return Ok(());
    }
    let mut ranges = cx.doc().page_labels()?.ranges;
    if ranges.is_empty() {
        return Ok(());
    }
    for r in &mut ranges {
        if r.start_page > 0 && r.start_page >= at {
            r.start_page += count;
        }
    }
    replace(cx, ranges)
}

/// The pages in `deleted` (sorted, unique, old numbering) were removed from a document of
/// `old_count` pages. Ranges that lose all their pages disappear; the rest start at the same
/// content.
pub(crate) fn shift_for_delete(
    cx: &mut EditContext<'_>,
    deleted: &[usize],
    old_count: usize,
) -> Result<()> {
    if deleted.is_empty() || !has_labels(cx)? {
        return Ok(());
    }
    let ranges = cx.doc().page_labels()?.ranges;
    if ranges.is_empty() {
        return Ok(());
    }
    let before = |p: usize| deleted.partition_point(|&d| d < p);
    let mut out: Vec<PageLabelRange> = vec![];
    for (i, r) in ranges.iter().enumerate() {
        let end = ranges.get(i + 1).map_or(old_count, |n| n.start_page);
        let survivors = (end - r.start_page.min(end)) - (before(end) - before(r.start_page));
        if survivors == 0 {
            continue;
        }
        let mut n = r.clone();
        n.start_page = r.start_page - before(r.start_page);
        out.push(n);
    }
    if let Some(f) = out.first_mut() {
        f.start_page = 0;
    }
    replace(cx, out)
}
