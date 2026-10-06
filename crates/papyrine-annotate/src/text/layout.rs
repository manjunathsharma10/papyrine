//! Text layout for text boxes: bidi, font fallback, shaping (rustybuzz), greedy line breaking.

use std::collections::BTreeMap;

use rustybuzz::{Direction, UnicodeBuffer};
use unicode_bidi::BidiInfo;

use super::fonts::{FontFace, FontReport, FontSet};
use crate::geometry::{FontFamily, TextStyle};

#[derive(Clone, Debug)]
pub struct PlacedGlyph {
    /// Index into [`Layout::faces`].
    pub face: usize,
    /// Glyph id in the original font.
    pub gid: u16,
    /// Pen position before this glyph, relative to the line start (points).
    pub x: f64,
    /// The shaper's offsets from the pen position (marks and positioned glyphs), points.
    pub dx: f64,
    pub dy: f64,
    /// Advance chosen by the shaper (kerning included).
    pub advance: f64,
    /// Text this glyph stands for (for ToUnicode), on the first glyph of each cluster.
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    /// In visual (left-to-right) order.
    pub glyphs: Vec<PlacedGlyph>,
    pub width: f64,
    /// The line's text in logical order.
    pub text: String,
    pub has_rtl: bool,
}

pub struct Layout {
    pub faces: Vec<FontFace>,
    pub lines: Vec<Line>,
    pub ascent: f64,
    pub descent: f64,
    pub line_height: f64,
    pub report: FontReport,
}

impl Layout {
    pub fn text_height(&self) -> f64 {
        self.lines.len() as f64 * self.line_height
    }
    pub fn max_line_width(&self) -> f64 {
        self.lines.iter().map(|l| l.width).fold(0.0, f64::max)
    }
    /// Glyphs used per face: gid -> text.
    pub fn used_glyphs(&self) -> Vec<BTreeMap<u16, String>> {
        let mut used = vec![BTreeMap::new(); self.faces.len()];
        for l in &self.lines {
            for g in &l.glyphs {
                let e = used[g.face].entry(g.gid).or_insert_with(String::new);
                if e.is_empty()
                    && let Some(t) = &g.text
                {
                    *e = t.clone();
                }
            }
        }
        used
    }
}

#[derive(Clone)]
struct Seg {
    level: u8,
    glyphs: Vec<PlacedGlyph>,
    width: f64,
    text: String,
}

#[derive(Clone)]
struct Token {
    space: bool,
    segs: Vec<Seg>,
    width: f64,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x303F | 0x3040..=0x30FF | 0x3100..=0x312F | 0x3130..=0x318F | 0x31F0..=0x31FF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

fn is_ignorable(c: char) -> bool {
    matches!(c as u32,
        0x034F | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFE00..=0xFE0F | 0xFEFF)
}

/// Lay `text` out in lines no wider than `width` points (a hard `\n` always breaks).
pub fn layout(text: &str, style: &TextStyle, width: f64) -> Layout {
    layout_with(text, style.family, style.bold, style.size, width, true)
}

/// As [`layout`]; with `system` false only the bundled fonts are used (deterministic tests).
pub fn layout_with(
    text: &str,
    family: FontFamily,
    bold: bool,
    size: f64,
    width: f64,
    system: bool,
) -> Layout {
    let mut set = FontSet::new(family, bold, system);
    let norm = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', " ");
    let paras: Vec<&str> = norm.split('\n').collect();

    // Pass 1: choose a face for every character (may add system fallback faces).
    let mut para_faces: Vec<Vec<usize>> = Vec::new();
    for p in &paras {
        let mut v = Vec::with_capacity(p.chars().count());
        let mut prev = 0usize;
        for c in p.chars() {
            let f = if is_ignorable(c) {
                prev
            } else {
                set.face_for(c)
            };
            prev = f;
            v.push(f);
        }
        para_faces.push(v);
    }
    let faces = set.faces.clone();
    let report = set.report.clone();
    let rb: Vec<rustybuzz::Face<'_>> = faces
        .iter()
        .map(|f| {
            rustybuzz::Face::from_slice(f.bytes(), f.index)
                .or_else(|| {
                    rustybuzz::Face::from_slice(super::fonts::bundled_static(family, bold), 0)
                })
                .expect("bundled font parses")
        })
        .collect();

    let upem = f64::from(rb[0].units_per_em().max(1));
    let k = size / upem;
    let ascent = f64::from(rb[0].ascender()) * k;
    let descent = f64::from(rb[0].descender()) * k; // negative
    let gap = f64::from(rb[0].line_gap()) * k;
    let line_height = (ascent - descent + gap).max(size);

    let mut lines: Vec<Line> = Vec::new();
    for (p, fidx) in paras.iter().zip(&para_faces) {
        if p.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let tokens = tokenize(p, fidx, &rb, size, width);
        wrap(tokens, width, &mut lines);
    }
    Layout {
        faces,
        lines,
        ascent,
        descent,
        line_height,
        report,
    }
}

/// Split a paragraph into word / space tokens and shape each.
fn tokenize(
    para: &str,
    fidx: &[usize],
    rb: &[rustybuzz::Face<'_>],
    size: f64,
    width: f64,
) -> Vec<Token> {
    let bidi = BidiInfo::new(para, None);
    let chars: Vec<(usize, char)> = para.char_indices().collect();
    // Token boundaries as char index ranges.
    let mut ranges: Vec<(usize, usize, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if c == ' ' {
            let s = i;
            while i < chars.len() && chars[i].1 == ' ' {
                i += 1;
            }
            ranges.push((s, i, true));
        } else if is_cjk(c) {
            ranges.push((i, i + 1, false));
            i += 1;
        } else {
            let s = i;
            while i < chars.len() && chars[i].1 != ' ' && !is_cjk(chars[i].1) {
                i += 1;
            }
            ranges.push((s, i, false));
        }
    }
    let mut out = Vec::new();
    for (s, e, space) in ranges {
        let tok = shape_range(para, &bidi, &chars, fidx, rb, size, s, e, space);
        if !space && tok.width > width && e - s > 1 {
            // A word wider than the box: break it between characters.
            for ci in s..e {
                out.push(shape_range(
                    para,
                    &bidi,
                    &chars,
                    fidx,
                    rb,
                    size,
                    ci,
                    ci + 1,
                    false,
                ));
            }
        } else {
            out.push(tok);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn shape_range(
    para: &str,
    bidi: &BidiInfo<'_>,
    chars: &[(usize, char)],
    fidx: &[usize],
    rb: &[rustybuzz::Face<'_>],
    size: f64,
    s: usize,
    e: usize,
    space: bool,
) -> Token {
    let mut segs: Vec<Seg> = Vec::new();
    let mut i = s;
    while i < e {
        let level = bidi.levels[chars[i].0].number();
        let face = fidx[i];
        let mut j = i + 1;
        while j < e && bidi.levels[chars[j].0].number() == level && fidx[j] == face {
            j += 1;
        }
        let b0 = chars[i].0;
        let b1 = if j < chars.len() {
            chars[j].0
        } else {
            para.len()
        };
        let seg_text = &para[b0..b1];
        segs.push(shape_seg(&rb[face], face, seg_text, level, size));
        i = j;
    }
    let width = segs.iter().map(|g| g.width).sum();
    Token { space, segs, width }
}

fn shape_seg(face: &rustybuzz::Face<'_>, face_idx: usize, text: &str, level: u8, size: f64) -> Seg {
    let rtl = level % 2 == 1;
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    buf.set_direction(if rtl {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    });
    buf.guess_segment_properties();
    let out = rustybuzz::shape(face, &[], buf);
    let k = size / f64::from(face.units_per_em().max(1));
    let infos = out.glyph_infos();
    let pos = out.glyph_positions();
    // Cluster start offsets in ascending order, to cut the text per cluster.
    let mut starts: Vec<u32> = infos.iter().map(|g| g.cluster).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut claimed: Vec<u32> = Vec::new();
    let mut glyphs = Vec::with_capacity(infos.len());
    let mut pen = 0.0;
    for (info, p) in infos.iter().zip(pos) {
        let text_for = if claimed.contains(&info.cluster) {
            None
        } else {
            claimed.push(info.cluster);
            let a = info.cluster as usize;
            let b = starts
                .iter()
                .find(|&&c| c > info.cluster)
                .map_or(text.len(), |&c| c as usize);
            text.get(a..b).map(str::to_owned)
        };
        let adv = f64::from(p.x_advance) * k;
        glyphs.push(PlacedGlyph {
            face: face_idx,
            gid: info.glyph_id as u16,
            x: pen,
            dx: f64::from(p.x_offset) * k,
            dy: f64::from(p.y_offset) * k,
            advance: adv,
            text: text_for,
        });
        pen += adv;
    }
    Seg {
        level,
        glyphs,
        width: pen,
        text: text.to_owned(),
    }
}

fn wrap(tokens: Vec<Token>, width: f64, lines: &mut Vec<Line>) {
    let mut cur: Vec<Token> = Vec::new();
    let mut cur_w = 0.0;
    let mut cur_has_word = false;
    for t in tokens {
        if t.space {
            if !cur.is_empty() {
                cur_w += t.width;
                cur.push(t);
            }
            continue;
        }
        if cur_has_word && cur_w + t.width > width + 1e-6 {
            lines.push(finish_line(std::mem::take(&mut cur)));
            cur_w = 0.0;
        }
        cur_w += t.width;
        cur_has_word = true;
        cur.push(t);
    }
    lines.push(finish_line(cur));
}

fn finish_line(mut toks: Vec<Token>) -> Line {
    while toks.last().is_some_and(|t| t.space) {
        toks.pop();
    }
    let segs: Vec<&Seg> = toks.iter().flat_map(|t| t.segs.iter()).collect();
    let text: String = segs.iter().map(|s| s.text.as_str()).collect();
    let levels: Vec<u8> = segs.iter().map(|s| s.level).collect();
    let order = visual_order(&levels);
    let mut glyphs = Vec::new();
    let mut pen = 0.0;
    for &si in &order {
        let s = segs[si];
        for g in &s.glyphs {
            let mut g = g.clone();
            g.x += pen;
            glyphs.push(g);
        }
        pen += s.width;
    }
    Line {
        glyphs,
        width: pen,
        text,
        has_rtl: levels.iter().any(|l| l % 2 == 1),
    }
}

/// Rule L2 of the bidi algorithm over a sequence of segments: indices in visual order.
fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let max = levels.iter().copied().max().unwrap_or(0);
    let min_odd = levels.iter().copied().filter(|l| l % 2 == 1).min();
    let Some(min_odd) = min_odd else { return order };
    for lvl in (min_odd..=max).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] >= lvl {
                let s = i;
                while i < order.len() && levels[order[i]] >= lvl {
                    i += 1;
                }
                order[s..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(size: f64) -> TextStyle {
        TextStyle {
            size,
            ..TextStyle::default()
        }
    }

    #[test]
    fn wraps_at_spaces_and_respects_newlines() {
        let l = layout("hello world foo\nbar", &style(12.0), 60.0);
        assert!(
            l.lines.len() >= 3,
            "{:?}",
            l.lines.iter().map(|x| &x.text).collect::<Vec<_>>()
        );
        assert_eq!(l.lines.last().unwrap().text, "bar");
        for line in &l.lines {
            assert!(line.width <= 60.0 + 1e-6 || !line.text.contains(' '));
        }
    }

    #[test]
    fn long_word_breaks_between_chars() {
        let l = layout("abcdefghijklmnopqrstuvwxyz", &style(12.0), 40.0);
        assert!(l.lines.len() > 1);
        let joined: String = l.lines.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(joined, "abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn kerning_changes_advances() {
        let l = layout("AV", &style(100.0), 10_000.0);
        let g = &l.lines[0].glyphs;
        assert_eq!(g.len(), 2);
        // Noto Sans kerns A-V tightly: the pair is narrower than the sum of default advances.
        let face = rustybuzz::Face::from_slice(l.faces[0].bytes(), 0).unwrap();
        let default = |c: char| {
            let gid = face.glyph_index(c).unwrap();
            f64::from(face.glyph_hor_advance(gid).unwrap()) * 100.0 / f64::from(face.units_per_em())
        };
        assert!(l.lines[0].width < default('A') + default('V') - 1.0);
    }

    #[test]
    fn visual_order_reverses_rtl_runs() {
        assert_eq!(visual_order(&[0, 1, 1, 0]), vec![0, 2, 1, 3]);
        assert_eq!(visual_order(&[1, 1]), vec![1, 0]);
        assert_eq!(visual_order(&[0, 0]), vec![0, 1]);
        assert_eq!(visual_order(&[0, 1, 2, 1, 0]), vec![0, 3, 2, 1, 4]);
    }

    #[test]
    fn missing_chars_are_reported() {
        // A private-use character no font covers.
        let l = layout_with("a\u{E000}", FontFamily::Sans, false, 12.0, 100.0, false);
        assert_eq!(l.report.missing, "\u{E000}");
    }
}
