use papyrine_cos::Document;
use papyrine_ops::{
    EditContext, ImportOptions, OutlineMode, check_source_permissions, import_pages,
};

use crate::labels::LabelCarry;
use crate::merge::{InfoPolicy, carry_info, finish_catalog};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// Bookmarks that lead to the extracted pages come along (default), or none.
    pub outline: OutlineMode,
    pub named_dests: bool,
    pub page_labels: bool,
    /// Default: title, author, subject, keywords and language come from the source.
    pub info: InfoPolicy,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        ExtractOptions {
            outline: OutlineMode::Merged,
            named_dests: true,
            page_labels: true,
            info: InfoPolicy::First,
        }
    }
}

/// One output document of an extract or split.
pub struct Part {
    /// Zero-based part number.
    pub index: usize,
    /// Zero-based source pages, in the order they appear in the part.
    pub pages: Vec<usize>,
    pub doc: Document,
}

impl Part {
    /// One-based first and last source page (for file names).
    pub fn page_span(&self) -> (usize, usize) {
        let first = self.pages.iter().min().copied().unwrap_or(0);
        let last = self.pages.iter().max().copied().unwrap_or(0);
        (first + 1, last + 1)
    }
}

fn build_part(
    src: &Document,
    index: usize,
    pages: &[usize],
    opts: &ExtractOptions,
) -> Result<Part> {
    let out = Document::new_empty()?;
    let mut cx = EditContext::new(&out)?;
    let io = ImportOptions {
        outline: opts.outline,
        outline_title: String::new(),
        named_dests: opts.named_dests,
        place_by_page: false,
    };
    import_pages(&mut cx, src, pages, 0, &io)?;
    if opts.page_labels {
        let mut carry = LabelCarry::default();
        carry.add(&src.page_labels()?, pages);
        carry.finish(&out)?;
    }
    carry_info(&out, src, opts.info)?;
    finish_catalog(&out)?;
    Ok(Part {
        index,
        pages: pages.to_vec(),
        doc: out,
    })
}

fn check_pages(src: &Document, pages: &[usize]) -> Result<()> {
    let n = src.page_count()?;
    if pages.is_empty() {
        return Err(Error::invalid("no pages selected"));
    }
    if let Some(&bad) = pages.iter().find(|&&p| p >= n) {
        return Err(Error::invalid(format!(
            "page {} is out of range (document has {n})",
            bad + 1
        )));
    }
    Ok(())
}

/// The selected pages (zero-based, in the order given) as one new document.
pub fn extract(src: &Document, pages: &[usize], opts: &ExtractOptions) -> Result<Document> {
    check_source_permissions(src)?;
    check_pages(src, pages)?;
    Ok(build_part(src, 0, pages, opts)?.doc)
}

/// Lazily builds one document per group of pages, so a long split never holds more than the
/// part being written.
pub struct PartIter<'a> {
    src: &'a Document,
    groups: Vec<Vec<usize>>,
    next: usize,
    opts: ExtractOptions,
}

impl PartIter<'_> {
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

impl Iterator for PartIter<'_> {
    type Item = Result<Part>;

    fn next(&mut self) -> Option<Self::Item> {
        let pages = self.groups.get(self.next)?;
        let r = build_part(self.src, self.next, pages, &self.opts);
        self.next += 1;
        Some(r)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.groups.len() - self.next;
        (n, Some(n))
    }
}

fn parts<'a>(
    src: &'a Document,
    groups: Vec<Vec<usize>>,
    opts: &ExtractOptions,
) -> Result<PartIter<'a>> {
    check_source_permissions(src)?;
    for g in &groups {
        check_pages(src, g)?;
    }
    Ok(PartIter {
        src,
        groups,
        next: 0,
        opts: opts.clone(),
    })
}

/// One document per selected page, in the order given.
pub fn extract_each<'a>(
    src: &'a Document,
    pages: &[usize],
    opts: &ExtractOptions,
) -> Result<PartIter<'a>> {
    parts(src, pages.iter().map(|&p| vec![p]).collect(), opts)
}

/// One document per inclusive zero-based `(first, last)` range, in the order given. Ranges
/// may not overlap; pages between ranges are left out.
pub fn split_ranges<'a>(
    src: &'a Document,
    ranges: &[(usize, usize)],
    opts: &ExtractOptions,
) -> Result<PartIter<'a>> {
    let mut sorted: Vec<(usize, usize)> = ranges.to_vec();
    sorted.sort_unstable();
    for r in &sorted {
        if r.0 > r.1 {
            return Err(Error::invalid(format!(
                "range {}-{} runs backwards",
                r.0 + 1,
                r.1 + 1
            )));
        }
    }
    if let Some(w) = sorted.windows(2).find(|w| w[0].1 >= w[1].0) {
        return Err(Error::invalid(format!(
            "ranges {}-{} and {}-{} overlap",
            w[0].0 + 1,
            w[0].1 + 1,
            w[1].0 + 1,
            w[1].1 + 1
        )));
    }
    parts(
        src,
        ranges.iter().map(|&(a, b)| (a..=b).collect()).collect(),
        opts,
    )
}

/// Consecutive documents of `n` pages each (the last may be shorter).
pub fn split_every<'a>(src: &'a Document, n: usize, opts: &ExtractOptions) -> Result<PartIter<'a>> {
    if n == 0 {
        return Err(Error::invalid("part size must be at least one page"));
    }
    let count = src.page_count()?;
    let groups = (0..count)
        .step_by(n)
        .map(|s| (s..(s + n).min(count)).collect())
        .collect();
    parts(src, groups, opts)
}

/// Zero-based inclusive `(first, last)` ranges from text such as `1-3, 4, 6-` (one-based pages,
/// open ends allowed); one part per item, in the order written.
pub fn parse_split_ranges(text: &str, page_count: usize) -> Result<Vec<(usize, usize)>> {
    Ok(papyrine_ops::parse_split_ranges(text, page_count)?)
}
