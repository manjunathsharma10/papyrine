use crate::normalize::{NormOpts, Normalized, is_combining, normalize};
use crate::quads::{Quad, line_quads};
use papyrine_core::{CancelToken, Rect};
use std::ops::Range;
use std::time::{Duration, Instant};

/// Order of the supplied chars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BidiOrder {
    /// Chars are in logical (reading) order. This is what PDFium returns for
    /// Arabic and Hebrew (verified on the corpus), so it is the default.
    #[default]
    Logical,
    /// Chars are in visual (left-to-right on the page) order; RTL runs are
    /// reversed back to logical order per line before matching.
    Visual,
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub whole_word: bool,
    pub case_sensitive: bool,
    pub diacritic_insensitive: bool,
    /// Turkish/Azeri casing: I<->ı and İ<->i instead of I<->i. Only matters
    /// when `case_sensitive` is false. Wired to the UI locale by the app.
    pub turkish_case: bool,
    pub bidi: BidiOrder,
    /// Chars of context kept on each side of the match in [`Snippet`].
    pub snippet_context: usize,
    /// Stop after this many hits in total.
    pub max_hits: Option<usize>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            whole_word: false,
            case_sensitive: false,
            diacritic_insensitive: false,
            turkish_case: false,
            bidi: BidiOrder::Logical,
            snippet_context: 40,
            max_hits: None,
        }
    }
}

impl SearchOptions {
    fn norm(&self) -> NormOpts {
        NormOpts {
            fold_case: !self.case_sensitive,
            strip_marks: self.diacritic_insensitive,
            turkish: self.turkish_case,
        }
    }
}

/// Page text as the renderer reports it: `boxes[i]` is the highlight box of
/// `chars[i]` in page user space. Generated chars (the `\r\n` between lines)
/// may have empty boxes.
#[derive(Clone, Debug, Default)]
pub struct PageChars {
    pub chars: Vec<char>,
    pub boxes: Vec<Rect>,
}

impl PageChars {
    pub fn new(chars: Vec<char>, boxes: Vec<Rect>) -> Self {
        PageChars { chars, boxes }
    }

    /// Text with no geometry (hits will have no quads).
    pub fn from_text(text: &str) -> Self {
        PageChars {
            chars: text.chars().collect(),
            boxes: Vec::new(),
        }
    }
}

/// Where a hit's text came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSource {
    PageText,
    /// Annotation contents; the string is the caller's annotation id.
    Annotation(String),
    /// Form field value; the string is the field's fully qualified name.
    FormField(String),
}

/// Extra searchable text attached to a page (annotation contents, form values).
#[derive(Clone, Debug)]
pub struct ExtraText {
    pub source: TextSource,
    pub text: String,
    /// Highlighted for every hit in this text (annotation rect, widget rect).
    pub anchor: Option<Rect>,
}

#[derive(Clone, Debug, Default)]
pub struct PageInput {
    pub page: usize,
    pub text: PageChars,
    pub extras: Vec<ExtraText>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub before: String,
    pub matched: String,
    pub after: String,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub page: usize,
    pub source: TextSource,
    /// Hit number within this page and source, from 0.
    pub ordinal: usize,
    /// Char index range into [`PageChars::chars`] (or the extra text's chars).
    pub range: Range<usize>,
    /// One merged quad per line.
    pub quads: Vec<Quad>,
    pub snippet: Snippet,
}

#[derive(Clone, Debug)]
pub enum SearchEvent {
    Hit(SearchHit),
    PageDone { page: usize, hits: usize },
}

#[derive(Clone, Debug, Default)]
pub struct SearchSummary {
    pub pages_searched: usize,
    pub hits: usize,
    /// Time from the call to the first emitted hit.
    pub first_hit_at: Option<Duration>,
    pub elapsed: Duration,
    pub cancelled: bool,
    /// `max_hits` was reached.
    pub truncated: bool,
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Searcher {
    needle: Vec<char>,
    opts: SearchOptions,
}

impl Searcher {
    /// `None` when the query is empty after normalization.
    pub fn new(query: &str, opts: &SearchOptions) -> Option<Searcher> {
        let q: Vec<char> = query.chars().collect();
        let mut n = normalize(&q, opts.norm(), false).chars;
        while n.last() == Some(&' ') {
            n.pop();
        }
        let lead = n.iter().take_while(|&&c| c == ' ').count();
        n.drain(..lead);
        (!n.is_empty()).then(|| Searcher {
            needle: n,
            opts: opts.clone(),
        })
    }

    /// Search one page (text then extras), calling `emit` per hit as it is
    /// found. `emit` returns `false` to stop. Returns `false` if stopped.
    pub fn search_page(&self, input: &PageInput, emit: &mut dyn FnMut(SearchHit) -> bool) -> bool {
        let visual = self.opts.bidi == BidiOrder::Visual;
        if !self.search_chars(
            input.page,
            &TextSource::PageText,
            &input.text.chars,
            &input.text.boxes,
            None,
            visual,
            emit,
        ) {
            return false;
        }
        for x in &input.extras {
            let chars: Vec<char> = x.text.chars().collect();
            if !self.search_chars(input.page, &x.source, &chars, &[], x.anchor, false, emit) {
                return false;
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn search_chars(
        &self,
        page: usize,
        source: &TextSource,
        chars: &[char],
        boxes: &[Rect],
        anchor: Option<Rect>,
        visual: bool,
        emit: &mut dyn FnMut(SearchHit) -> bool,
    ) -> bool {
        let n = self.needle.len();
        let hay = normalize(chars, self.opts.norm(), visual);
        let h = &hay.chars;
        let first = self.needle[0];
        let bounds = self.opts.whole_word.then(|| word_bounds(h));
        let mut ordinal = 0;
        let mut j = 0;
        while j + n <= h.len() {
            match h[j..=h.len() - n].iter().position(|&c| c == first) {
                Some(p) => j += p,
                None => break,
            }
            if h[j..j + n] == self.needle[..] && self.boundary_ok(h, j, n, bounds.as_deref()) {
                let range = src_range(&hay, j, n);
                let quads = match anchor {
                    Some(a) => vec![Quad::from_rect(a)],
                    None => line_quads(chars, boxes, range.clone()),
                };
                let snippet = make_snippet(chars, &range, self.opts.snippet_context);
                let hit = SearchHit {
                    page,
                    source: source.clone(),
                    ordinal,
                    range,
                    quads,
                    snippet,
                };
                ordinal += 1;
                if !emit(hit) {
                    return false;
                }
                j += n;
            } else {
                j += 1;
            }
        }
        true
    }

    fn boundary_ok(&self, h: &[char], j: usize, n: usize, bounds: Option<&[bool]>) -> bool {
        // A match must not start or end inside a base+mark cluster...
        if is_combining(h[j]) {
            return false;
        }
        let last = h[j + n - 1];
        if let Some(&next) = h.get(j + n) {
            if is_combining(next) {
                return false;
            }
            // ...nor inside a Hangul syllable (NFKD splits it into jamo).
            if ('\u{1161}'..='\u{11c2}').contains(&next)
                && ('\u{1100}'..='\u{11c2}').contains(&last)
            {
                return false;
            }
        }
        if ('\u{1161}'..='\u{11c2}').contains(&h[j])
            && j > 0
            && ('\u{1100}'..='\u{11c2}').contains(&h[j - 1])
        {
            return false;
        }
        bounds.is_none_or(|b| b[j] && b[j + n])
    }
}

/// `bounds[i]` is true when a UAX #29 word boundary lies before char `i`.
fn word_bounds(h: &[char]) -> Vec<bool> {
    use unicode_segmentation::UnicodeSegmentation;
    let s: String = h.iter().collect();
    let mut b = vec![false; h.len() + 1];
    b[0] = true;
    let mut ci = 0;
    for w in s.split_word_bounds() {
        ci += w.chars().count();
        b[ci] = true;
    }
    b
}

fn src_range(hay: &Normalized, j: usize, n: usize) -> Range<usize> {
    let s = &hay.src[j..j + n];
    let lo = s.iter().map(|p| p.0).min().unwrap_or(0);
    let hi = s.iter().map(|p| p.1).max().unwrap_or(0);
    lo as usize..hi as usize
}

fn clean(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut space = false;
    for &c in chars {
        if matches!(c, '\u{ad}' | '\u{2}' | '\u{200b}'..='\u{200f}' | '\u{feff}') {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    if space {
        out.push(' ');
    }
    out
}

fn make_snippet(chars: &[char], r: &Range<usize>, ctx: usize) -> Snippet {
    let r = r.start.min(chars.len())..r.end.min(chars.len());
    let mut b0 = r.start.saturating_sub(ctx);
    // Snap a mid-word cut forward to the next word start (within a few chars).
    if b0 > 0
        && chars[b0 - 1].is_alphanumeric()
        && chars[b0].is_alphanumeric()
        && let Some(p) = chars[b0..r.start]
            .iter()
            .take(12)
            .position(|c| c.is_whitespace())
    {
        b0 += p + 1;
    }
    let mut a1 = (r.end + ctx).min(chars.len());
    if a1 < chars.len()
        && a1 > r.end
        && chars[a1 - 1].is_alphanumeric()
        && chars[a1].is_alphanumeric()
        && let Some(p) = chars[r.end..a1]
            .iter()
            .rev()
            .take(12)
            .position(|c| c.is_whitespace())
    {
        a1 -= p + 1;
    }
    Snippet {
        before: clean(&chars[b0..r.start]).trim_start().to_string(),
        matched: clean(&chars[r.clone()]).trim().to_string(),
        after: clean(&chars[r.end..a1]).trim_end().to_string(),
    }
}

/// Search pages as they are produced, streaming hits to `sink`.
///
/// `pages` is pulled lazily, one page at a time, so extraction of page N+1
/// never starts before page N's hits were delivered: the first hit arrives as
/// soon as its page has been extracted and normalized. `cancel` is checked
/// before every page pull and between hits.
pub fn search_document<I, F>(
    pages: I,
    query: &str,
    opts: &SearchOptions,
    cancel: &CancelToken,
    mut sink: F,
) -> SearchSummary
where
    I: IntoIterator<Item = PageInput>,
    F: FnMut(SearchEvent),
{
    let t0 = Instant::now();
    let mut sum = SearchSummary::default();
    let Some(s) = Searcher::new(query, opts) else {
        return sum;
    };
    let mut it = pages.into_iter();
    'pages: loop {
        if cancel.is_cancelled() {
            sum.cancelled = true;
            break;
        }
        let Some(page) = it.next() else { break };
        let mut page_hits = 0;
        let ok = s.search_page(&page, &mut |hit| {
            if cancel.is_cancelled() {
                sum.cancelled = true;
                return false;
            }
            if opts.max_hits.is_some_and(|m| sum.hits >= m) {
                sum.truncated = true;
                return false;
            }
            sum.first_hit_at.get_or_insert_with(|| t0.elapsed());
            sum.hits += 1;
            page_hits += 1;
            sink(SearchEvent::Hit(hit));
            true
        });
        if !ok {
            break 'pages;
        }
        sum.pages_searched += 1;
        sink(SearchEvent::PageDone {
            page: page.page,
            hits: page_hits,
        });
        if opts.max_hits.is_some_and(|m| sum.hits >= m) {
            sum.truncated = true;
            break;
        }
    }
    sum.elapsed = t0.elapsed();
    sum
}
