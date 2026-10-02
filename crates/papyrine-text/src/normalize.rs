//! Text normalization for search: char stream -> normalized char stream with a
//! source-range map back to the input indices.
//!
//! Pipeline (ADR: both query and page text go through exactly the same steps):
//! 1. optional visual -> logical bidi reordering (per line),
//! 2. drop ignorables (soft hyphen, zero-width and bidi controls, tatweel),
//! 3. join line-end hyphenation, collapse whitespace runs to one space,
//! 4. NFKD per char (expands ligatures such as U+FB01 and the Arabic/Latin
//!    presentation forms), optional diacritic strip, optional case fold,
//! 5. canonical reordering of combining marks.
//!
//! We compare in NFKD rather than NFKC so that every normalized char maps to
//! exactly one input char; the matcher refuses to split a base+mark cluster.

use unicode_normalization::char::{canonical_combining_class, decompose_compatible};

#[derive(Clone, Copy, Debug)]
pub(crate) struct NormOpts {
    pub fold_case: bool,
    pub strip_marks: bool,
    pub turkish: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Normalized {
    pub chars: Vec<char>,
    /// Input index range `[start, end)` that produced each normalized char.
    pub src: Vec<(u32, u32)>,
}

impl Normalized {
    fn push(&mut self, c: char, s: u32, e: u32) {
        self.chars.push(c);
        self.src.push((s, e));
    }
}

/// Soft hyphen, and the U+0002 PDFium emits for a hyphen it generated at a
/// line break.
fn is_soft_hyphen(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{2}')
}

fn is_ignorable(c: char) -> bool {
    matches!(c,
        '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{feff}' | '\u{61c}' | '\u{640}' | '\u{34f}')
}

fn is_ws(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{a0}' | '\u{2007}' | '\u{202f}')
}

/// Scripts written without spaces between words (Hangul is spaced).
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x30ff | 0x31f0..=0x31ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
        | 0xf900..=0xfaff | 0xff00..=0xffef | 0x20000..=0x2fa1f)
}

fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{c}' | '\u{b}')
}

/// Any combining mark (a base+mark cluster must not be split by a match).
pub(crate) fn is_combining(c: char) -> bool {
    !c.is_ascii()
        && (canonical_combining_class(c) != 0
            // Thai and Lao above/below vowels and tone marks (class 0 but Mn).
            || matches!(c, '\u{e31}' | '\u{e34}'..='\u{e3a}' | '\u{e47}'..='\u{e4e}'
                | '\u{eb1}' | '\u{eb4}'..='\u{ebc}' | '\u{ec8}'..='\u{ecd}'))
}

/// Marks removed in diacritic-insensitive mode. Kana voiced-sound marks are
/// kept: が and か are different letters, not an accent.
fn is_strippable(c: char) -> bool {
    is_combining(c) && !matches!(c, '\u{3099}' | '\u{309a}')
}

fn fold_char(c: char, mut f: impl FnMut(char)) {
    match c {
        'ß' | 'ẞ' => {
            f('s');
            f('s');
        }
        'ς' => f('σ'),
        'ſ' => f('s'),
        _ => c.to_lowercase().for_each(f),
    }
}

/// Reorder `input` (a line-by-line *visual* sequence) into logical order and
/// return `(char, original index)` pairs.
fn visual_to_logical(input: &[char]) -> Vec<(char, u32)> {
    use unicode_bidi::BidiInfo;
    let mut out = Vec::with_capacity(input.len());
    let mut start = 0;
    while start < input.len() {
        let mut end = start;
        while end < input.len() && !is_newline(input[end]) {
            end += 1;
        }
        let line = &input[start..end];
        if line.iter().any(|&c| !c.is_ascii()) {
            let text: String = line.iter().collect();
            // byte offset -> char index within the line
            let mut byte_to_char = vec![0usize; text.len() + 1];
            for (ci, (bi, c)) in text.char_indices().enumerate() {
                byte_to_char[bi] = ci;
                byte_to_char[bi + c.len_utf8()] = ci + 1;
            }
            let info = BidiInfo::new(&text, None);
            for para in &info.paragraphs {
                let (levels, runs) = info.visual_runs(para, para.range.clone());
                for run in runs {
                    let (a, b) = (byte_to_char[run.start], byte_to_char[run.end]);
                    let rtl = levels[run.start].is_rtl();
                    let idx: Box<dyn Iterator<Item = usize>> = if rtl {
                        Box::new((a..b).rev())
                    } else {
                        Box::new(a..b)
                    };
                    for k in idx {
                        out.push((line[k], (start + k) as u32));
                    }
                }
            }
        } else {
            out.extend(
                line.iter()
                    .enumerate()
                    .map(|(k, &c)| (c, (start + k) as u32)),
            );
        }
        // newline chars themselves pass through in place
        let mut nl = end;
        while nl < input.len() && is_newline(input[nl]) {
            out.push((input[nl], nl as u32));
            nl += 1;
        }
        start = nl;
    }
    out
}

pub(crate) fn normalize(input: &[char], opts: NormOpts, visual_bidi: bool) -> Normalized {
    let seq: Vec<(char, u32)> = if visual_bidi {
        visual_to_logical(input)
    } else {
        input
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, i as u32))
            .collect()
    };
    let units = join_and_collapse(&seq);

    let mut out = Normalized {
        chars: Vec::with_capacity(units.len()),
        src: Vec::with_capacity(units.len()),
    };
    let mut any_marks = false;
    for (c, s, e) in units {
        if c.is_ascii() {
            let c = if opts.fold_case {
                if opts.turkish && c == 'I' {
                    'ı'
                } else {
                    c.to_ascii_lowercase()
                }
            } else {
                c
            };
            out.push(c, s, e);
            continue;
        }
        if opts.fold_case && opts.turkish && c == 'İ' {
            out.push('i', s, e);
            continue;
        }
        decompose_compatible(c, |d| {
            if is_combining(d) {
                if opts.strip_marks && is_strippable(d) {
                    return;
                }
                any_marks = true;
            }
            if opts.fold_case {
                fold_char(d, |f| {
                    if opts.strip_marks && is_strippable(f) {
                        return;
                    }
                    any_marks |= is_combining(f);
                    out.push(f, s, e);
                });
            } else {
                out.push(d, s, e);
            }
        });
    }
    if any_marks {
        reorder_marks(&mut out);
    }
    out
}

/// Drop ignorables, join hyphenated line ends, collapse whitespace.
/// Returns `(char, src_start, src_end)` units.
fn join_and_collapse(seq: &[(char, u32)]) -> Vec<(char, u32, u32)> {
    let mut units: Vec<(char, u32, u32)> = Vec::with_capacity(seq.len());
    let n = seq.len();
    let mut i = 0;
    // Index after `from` of a whitespace run containing a newline, followed by
    // a char satisfying `pred`: returns the index of that next char.
    let line_end_continuation = |from: usize, pred: &dyn Fn(char) -> bool| -> bool {
        let mut j = from;
        let mut saw_nl = false;
        while j < n && is_ws(seq[j].0) {
            saw_nl |= is_newline(seq[j].0);
            j += 1;
        }
        saw_nl && j < n && pred(seq[j].0)
    };
    while i < n {
        let (c, pos) = seq[i];
        if is_soft_hyphen(c) {
            // Drop it, and the line break after it if a word continues.
            let prev_alpha = units.last().is_some_and(|u| u.0.is_alphabetic());
            if prev_alpha && line_end_continuation(i + 1, &|c| c.is_alphabetic()) {
                i += 1;
                while i < n && is_ws(seq[i].0) {
                    i += 1;
                }
            } else {
                i += 1;
            }
            continue;
        }
        if is_ignorable(c) {
            i += 1;
            continue;
        }
        if matches!(c, '-' | '\u{2010}') {
            let prev_alpha = units.last().is_some_and(|u| u.0.is_alphabetic());
            if prev_alpha && line_end_continuation(i + 1, &|c| c.is_lowercase()) {
                i += 1;
                while i < n && is_ws(seq[i].0) {
                    i += 1;
                }
                continue;
            }
        }
        if is_ws(c) || (c.is_control() && !c.is_ascii_graphic()) {
            let (mut lo, mut hi) = (pos, pos + 1);
            let mut saw_nl = is_newline(c);
            i += 1;
            while i < n && (is_ws(seq[i].0) || is_ignorable(seq[i].0)) {
                lo = lo.min(seq[i].1);
                hi = hi.max(seq[i].1 + 1);
                saw_nl |= is_newline(seq[i].0);
                i += 1;
            }
            // A line break between two CJK chars is not a word separator.
            if saw_nl
                && units.last().is_some_and(|u| is_cjk(u.0))
                && seq.get(i).is_some_and(|n| is_cjk(n.0))
            {
                continue;
            }
            if let Some(last) = units.last_mut().filter(|u| u.0 == ' ') {
                last.1 = last.1.min(lo);
                last.2 = last.2.max(hi);
            } else {
                units.push((' ', lo, hi));
            }
            continue;
        }
        units.push((c, pos, pos + 1));
        i += 1;
    }
    units
}

/// Canonical ordering: stable-sort each run of combining marks by class.
fn reorder_marks(n: &mut Normalized) {
    let mut i = 0;
    while i < n.chars.len() {
        if !is_combining(n.chars[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n.chars.len() && is_combining(n.chars[j]) {
            j += 1;
        }
        if j - i > 1 {
            let mut run: Vec<(char, (u32, u32))> = (i..j).map(|k| (n.chars[k], n.src[k])).collect();
            run.sort_by_key(|&(c, _)| canonical_combining_class(c));
            for (k, (c, s)) in run.into_iter().enumerate() {
                n.chars[i + k] = c;
                n.src[i + k] = s;
            }
        }
        i = j;
    }
}
