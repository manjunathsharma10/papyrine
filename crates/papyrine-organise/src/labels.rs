//! Page labels for a document assembled from pages of several sources: every page keeps the
//! label it had in its source (style, prefix and number), and consecutive pages whose labels
//! continue each other share a range.

use papyrine_cos::{Document, LabelStyle, PageLabelRange, PageLabels};

use crate::Result;

/// `(style, prefix, number)` of a page as its source labels it. Without any range a page is
/// labelled with its decimal number, as viewers do.
fn label_of(l: &PageLabels, page: usize) -> (LabelStyle, String, usize) {
    match l.ranges.iter().rev().find(|r| r.start_page <= page) {
        Some(r) => (
            r.style,
            r.prefix.clone(),
            r.first_value as usize + (page - r.start_page),
        ),
        None => (LabelStyle::Decimal, String::new(), page + 1),
    }
}

/// Collects the labels of the pages of a new document in order.
#[derive(Default)]
pub(crate) struct LabelCarry {
    seq: Vec<(LabelStyle, String, usize)>,
    /// Some source had real labels (otherwise nothing needs writing).
    any: bool,
}

impl LabelCarry {
    /// The next pages of the new document are `pages` (indices) of a source with `labels`.
    pub fn add(&mut self, labels: &PageLabels, pages: &[usize]) {
        self.any |= !labels.ranges.is_empty();
        for &p in pages {
            self.seq.push(label_of(labels, p));
        }
    }

    pub fn finish(self, out: &Document) -> Result<()> {
        if !self.any || self.seq.is_empty() {
            return Ok(());
        }
        let mut ranges: Vec<PageLabelRange> = vec![];
        let mut prev: Option<&(LabelStyle, String, usize)> = None;
        for (i, cur) in self.seq.iter().enumerate() {
            let continues = prev.is_some_and(|p| {
                p.0 == cur.0 && p.1 == cur.1 && (cur.0 == LabelStyle::None || cur.2 == p.2 + 1)
            });
            if !continues {
                ranges.push(PageLabelRange {
                    start_page: i,
                    style: cur.0,
                    prefix: cur.1.clone(),
                    first_value: cur.2.max(1) as u32,
                });
            }
            prev = Some(cur);
        }
        // A single plain decimal range from 1 is what a document without labels already shows.
        if ranges.len() == 1
            && ranges[0].style == LabelStyle::Decimal
            && ranges[0].prefix.is_empty()
            && ranges[0].first_value == 1
        {
            return Ok(());
        }
        out.set_page_labels(&PageLabels { ranges })?;
        Ok(())
    }
}
