//! What an annotation looks like geometrically, independent of its colour and metadata.

use papyrine_ops::{Error, Result};
use serde::{Deserialize, Serialize};

pub type Pt = [f64; 2];

/// One highlighted run as four corners in Acrobat's order: top-left, top-right, bottom-left,
/// bottom-right (the order every viewer actually uses, not the counter-clockwise order the
/// specification text describes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Quad(pub [Pt; 4]);

impl Quad {
    /// An axis-aligned quad covering `[x0, y0, x1, y1]`.
    pub fn from_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Quad {
        let (xa, xb) = (x0.min(x1), x0.max(x1));
        let (ya, yb) = (y0.min(y1), y0.max(y1));
        Quad([[xa, yb], [xb, yb], [xa, ya], [xb, ya]])
    }
    pub fn bounds(&self) -> [f64; 4] {
        bounds_of(self.0.iter().copied()).unwrap_or([0.0; 4])
    }
    pub fn flatten(&self) -> [f64; 8] {
        let p = &self.0;
        [
            p[0][0], p[0][1], p[1][0], p[1][1], p[2][0], p[2][1], p[3][0], p[3][1],
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkupKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl MarkupKind {
    pub fn subtype(self) -> &'static str {
        match self {
            MarkupKind::Highlight => "Highlight",
            MarkupKind::Underline => "Underline",
            MarkupKind::StrikeOut => "StrikeOut",
            MarkupKind::Squiggly => "Squiggly",
        }
    }
}

/// Line ending styles (`/LE`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    #[default]
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

impl LineEnding {
    pub fn name(self) -> &'static str {
        match self {
            LineEnding::None => "None",
            LineEnding::Square => "Square",
            LineEnding::Circle => "Circle",
            LineEnding::Diamond => "Diamond",
            LineEnding::OpenArrow => "OpenArrow",
            LineEnding::ClosedArrow => "ClosedArrow",
            LineEnding::Butt => "Butt",
            LineEnding::ROpenArrow => "ROpenArrow",
            LineEnding::RClosedArrow => "RClosedArrow",
            LineEnding::Slash => "Slash",
        }
    }
    pub fn from_name(n: &str) -> LineEnding {
        match n {
            "Square" => LineEnding::Square,
            "Circle" => LineEnding::Circle,
            "Diamond" => LineEnding::Diamond,
            "OpenArrow" => LineEnding::OpenArrow,
            "ClosedArrow" => LineEnding::ClosedArrow,
            "Butt" => LineEnding::Butt,
            "ROpenArrow" => LineEnding::ROpenArrow,
            "RClosedArrow" => LineEnding::RClosedArrow,
            "Slash" => LineEnding::Slash,
            _ => LineEnding::None,
        }
    }
    /// Endings whose interior `/IC` fills.
    pub fn is_closed(self) -> bool {
        matches!(
            self,
            LineEnding::Square
                | LineEnding::Circle
                | LineEnding::Diamond
                | LineEnding::ClosedArrow
                | LineEnding::RClosedArrow
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontFamily {
    #[default]
    Sans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

impl Align {
    pub fn q(self) -> i64 {
        match self {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        }
    }
    pub fn from_q(q: i64) -> Align {
        match q {
            1 => Align::Center,
            2 => Align::Right,
            _ => Align::Left,
        }
    }
}

/// How the text of a text box is typeset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub family: FontFamily,
    pub bold: bool,
    pub size: f64,
    /// Text colour (DA operator); black by default.
    pub color: crate::Color,
    pub align: Align,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            family: FontFamily::Sans,
            bold: false,
            size: 12.0,
            color: crate::Color::gray(0.0),
            align: Align::Left,
        }
    }
}

/// Geometry of one annotation, with page-space coordinates (points, origin bottom-left, before
/// page rotation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    /// Highlight, underline, strike-out or squiggly over one or more quads.
    TextMarkup {
        kind: MarkupKind,
        quads: Vec<Quad>,
    },
    /// Sticky note whose top-left corner is `pos`; `icon` is `/Name` (Note, Comment, Help, Key...).
    Note {
        pos: Pt,
        icon: String,
    },
    /// Free text annotation with its text in `/Contents` (taken from the props).
    TextBox {
        rect: [f64; 4],
        style: TextStyle,
    },
    /// Pen strokes.
    Ink {
        strokes: Vec<Vec<Pt>>,
    },
    Square {
        rect: [f64; 4],
    },
    Circle {
        rect: [f64; 4],
    },
    Line {
        from: Pt,
        to: Pt,
        start: LineEnding,
        end: LineEnding,
    },
}

impl Geometry {
    pub fn highlight(quads: Vec<Quad>) -> Geometry {
        Geometry::TextMarkup {
            kind: MarkupKind::Highlight,
            quads,
        }
    }
    pub fn underline(quads: Vec<Quad>) -> Geometry {
        Geometry::TextMarkup {
            kind: MarkupKind::Underline,
            quads,
        }
    }
    pub fn strike_out(quads: Vec<Quad>) -> Geometry {
        Geometry::TextMarkup {
            kind: MarkupKind::StrikeOut,
            quads,
        }
    }
    pub fn squiggly(quads: Vec<Quad>) -> Geometry {
        Geometry::TextMarkup {
            kind: MarkupKind::Squiggly,
            quads,
        }
    }
    pub fn note(x: f64, y: f64) -> Geometry {
        Geometry::Note {
            pos: [x, y],
            icon: "Note".into(),
        }
    }
    pub fn text_box(rect: [f64; 4], style: TextStyle) -> Geometry {
        Geometry::TextBox { rect, style }
    }
    pub fn line(from: Pt, to: Pt) -> Geometry {
        Geometry::Line {
            from,
            to,
            start: LineEnding::None,
            end: LineEnding::None,
        }
    }
    /// A line with a closed arrow head at `to`.
    pub fn arrow(from: Pt, to: Pt) -> Geometry {
        Geometry::Line {
            from,
            to,
            start: LineEnding::None,
            end: LineEnding::ClosedArrow,
        }
    }

    pub fn subtype(&self) -> &'static str {
        match self {
            Geometry::TextMarkup { kind, .. } => kind.subtype(),
            Geometry::Note { .. } => "Text",
            Geometry::TextBox { .. } => "FreeText",
            Geometry::Ink { .. } => "Ink",
            Geometry::Square { .. } => "Square",
            Geometry::Circle { .. } => "Circle",
            Geometry::Line { .. } => "Line",
        }
    }

    /// True for a Line whose end is an arrow (the Arrow tool): it gets `/IT /LineArrow`.
    pub fn is_arrow(&self) -> bool {
        matches!(
            self,
            Geometry::Line { start: LineEnding::None, end, .. }
                if matches!(end, LineEnding::ClosedArrow | LineEnding::OpenArrow)
        )
    }

    pub fn validate(&self) -> Result<()> {
        let pt_ok = |p: &Pt| p.iter().all(|v| v.is_finite() && v.abs() < 1.0e7);
        let rect_ok = |r: &[f64; 4]| {
            r.iter().all(|v| v.is_finite() && v.abs() < 1.0e7)
                && (r[2] - r[0]).abs() >= 1.0
                && (r[3] - r[1]).abs() >= 1.0
        };
        match self {
            Geometry::TextMarkup { quads, .. } => {
                if quads.is_empty() || quads.len() > 100_000 {
                    return Err(Error::invalid("a text markup needs 1..=100000 quads"));
                }
                if !quads.iter().all(|q| q.0.iter().all(pt_ok)) {
                    return Err(Error::invalid("quad coordinates must be finite"));
                }
            }
            Geometry::Note { pos, .. } => {
                if !pt_ok(pos) {
                    return Err(Error::invalid("note position must be finite"));
                }
            }
            Geometry::TextBox { rect, style } => {
                if !rect_ok(rect) {
                    return Err(Error::invalid(
                        "text box must be finite and at least 1 pt wide and high",
                    ));
                }
                if !(style.size.is_finite() && (1.0..=500.0).contains(&style.size)) {
                    return Err(Error::invalid("font size must be within 1..=500"));
                }
                style.color.validate()?;
            }
            Geometry::Ink { strokes } => {
                let n: usize = strokes.iter().map(Vec::len).sum();
                if strokes.is_empty() || strokes.iter().any(Vec::is_empty) || n > 5_000_000 {
                    return Err(Error::invalid("ink needs non-empty strokes"));
                }
                if !strokes.iter().all(|s| s.iter().all(pt_ok)) {
                    return Err(Error::invalid("ink coordinates must be finite"));
                }
            }
            Geometry::Square { rect } | Geometry::Circle { rect } => {
                if !rect_ok(rect) {
                    return Err(Error::invalid(
                        "shape must be finite and at least 1 pt wide and high",
                    ));
                }
            }
            Geometry::Line { from, to, .. } => {
                if !pt_ok(from) || !pt_ok(to) {
                    return Err(Error::invalid("line endpoints must be finite"));
                }
                if (from[0] - to[0]).hypot(from[1] - to[1]) < 0.01 {
                    return Err(Error::invalid("a line needs two distinct endpoints"));
                }
            }
        }
        Ok(())
    }
}

pub fn bounds_of(pts: impl Iterator<Item = Pt>) -> Option<[f64; 4]> {
    let mut b: Option<[f64; 4]> = None;
    for p in pts {
        b = Some(match b {
            None => [p[0], p[1], p[0], p[1]],
            Some(b) => [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ],
        });
    }
    b
}

pub fn normalize_rect(r: [f64; 4]) -> [f64; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}
