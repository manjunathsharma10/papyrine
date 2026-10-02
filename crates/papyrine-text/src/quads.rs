//! Highlight geometry: merge per-char boxes of a match into one quad per line.

use papyrine_core::Rect;
use std::ops::Range;

/// A highlight quadrilateral in PDF `QuadPoints` order: top-left, top-right,
/// bottom-left, bottom-right, each `(x, y)` in page user space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad(pub [(f64, f64); 4]);

impl Quad {
    pub fn from_rect(r: Rect) -> Quad {
        let (x0, x1) = (r.x0.min(r.x1), r.x0.max(r.x1));
        let (y0, y1) = (r.y0.min(r.y1), r.y0.max(r.y1));
        Quad([(x0, y1), (x1, y1), (x0, y0), (x1, y0)])
    }

    /// Axis-aligned bounds.
    pub fn bounds(&self) -> Rect {
        let xs = self.0.map(|p| p.0);
        let ys = self.0.map(|p| p.1);
        let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
        let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
        Rect::new(min(xs), min(ys), max(xs), max(ys))
    }

    /// Flat `[x1 y1 x2 y2 x3 y3 x4 y4]` as written to an annotation.
    pub fn to_array(&self) -> [f64; 8] {
        let p = self.0;
        [
            p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1, p[3].0, p[3].1,
        ]
    }
}

fn norm(r: Rect) -> Rect {
    Rect::new(
        r.x0.min(r.x1),
        r.y0.min(r.y1),
        r.x0.max(r.x1),
        r.y0.max(r.y1),
    )
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

/// Fraction of the shorter extent covered by the overlap of two 1-D ranges.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    let min_len = (a1 - a0).min(b1 - b0);
    if min_len <= 1e-9 {
        // A zero-extent box counts as overlapping when it lies inside the other.
        let (p, lo, hi) = if a1 - a0 <= b1 - b0 {
            (a0, b0, b1)
        } else {
            (b0, a0, a1)
        };
        return if p >= lo - 1e-6 && p <= hi + 1e-6 {
            1.0
        } else {
            0.0
        };
    }
    ((a1.min(b1) - a0.max(b0)).max(0.0)) / min_len
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// One merged quad per visual line of `range`. Line breaks come from newline
/// chars and from geometry: consecutive boxes stay on a line while they overlap
/// by half or more on the cross axis of the run (the axis is inferred from the
/// first overlapping pair, so vertical CJK columns work too).
pub(crate) fn line_quads(chars: &[char], boxes: &[Rect], range: Range<usize>) -> Vec<Quad> {
    let mut out = Vec::new();
    let mut cur: Option<Rect> = None;
    let mut prev: Option<Rect> = None;
    let mut axis: Option<Axis> = None;
    for i in range {
        if matches!(chars[i], '\r' | '\n') {
            out.extend(cur.take().map(Quad::from_rect));
            prev = None;
            continue;
        }
        let Some(&raw) = boxes.get(i) else { continue };
        let b = norm(raw);
        if b.x1 - b.x0 <= 0.0 && b.y1 - b.y0 <= 0.0 {
            continue; // generated char with no geometry
        }
        if let Some(p) = prev {
            let yo = overlap(p.y0, p.y1, b.y0, b.y1);
            let xo = overlap(p.x0, p.x1, b.x0, b.x1);
            let same = match axis {
                Some(Axis::Horizontal) => yo >= 0.5,
                Some(Axis::Vertical) => xo >= 0.5,
                None if yo >= 0.5 && xo >= 0.5 => true,
                None if yo >= 0.5 => {
                    axis = Some(Axis::Horizontal);
                    true
                }
                None if xo >= 0.5 => {
                    axis = Some(Axis::Vertical);
                    true
                }
                None => false,
            };
            if !same {
                out.extend(cur.take().map(Quad::from_rect));
            }
        }
        cur = Some(cur.map_or(b, |c| union(c, b)));
        prev = Some(b);
    }
    out.extend(cur.map(Quad::from_rect));
    out
}
