//! Annotation properties shared by every annotation type.

use papyrine_content::ContentBuilder;
use serde::{Deserialize, Serialize};

use papyrine_ops::{Error, Result};

/// Annotation flag bits (`/F`, PDF 32000-1 table 165).
pub mod flags {
    pub const INVISIBLE: u32 = 1;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_ZOOM: u32 = 1 << 3;
    pub const NO_ROTATE: u32 = 1 << 4;
    pub const NO_VIEW: u32 = 1 << 5;
    pub const READ_ONLY: u32 = 1 << 6;
    pub const LOCKED: u32 = 1 << 7;
    /// Sticky notes and replies: printable, fixed size and orientation.
    pub const NOTE_DEFAULT: u32 = PRINT | NO_ZOOM | NO_ROTATE;
}

/// A device colour: 1 (gray), 3 (RGB) or 4 (CMYK) components in 0..=1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub Vec<f64>);

impl Color {
    pub fn rgb(r: f64, g: f64, b: f64) -> Color {
        Color(vec![r, g, b])
    }
    pub fn gray(g: f64) -> Color {
        Color(vec![g])
    }
    pub const fn black_rgb() -> [f64; 3] {
        [0.0, 0.0, 0.0]
    }
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.0.len(), 1 | 3 | 4) || self.0.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err(Error::invalid(
                "a colour needs 1, 3 or 4 components in 0..=1",
            ));
        }
        Ok(())
    }
    /// Components as read from `/C` or `/IC` (0 components means transparent).
    pub fn from_components(c: Vec<f64>) -> Option<Color> {
        let col = Color(c);
        col.validate().ok().map(|_| col)
    }
    /// Approximate RGB (for tests and UI previews).
    pub fn to_rgb(&self) -> [f64; 3] {
        match self.0.as_slice() {
            [g] => [*g, *g, *g],
            [r, g, b] => [*r, *g, *b],
            [c, m, y, k] => [
                (1.0 - c) * (1.0 - k),
                (1.0 - m) * (1.0 - k),
                (1.0 - y) * (1.0 - k),
            ],
            _ => Color::black_rgb(),
        }
    }
    pub(crate) fn set_fill(&self, b: &mut ContentBuilder) {
        match self.0.as_slice() {
            [g] => b.fill_gray(*g),
            [c, m, y, k] => b.fill_cmyk(*c, *m, *y, *k),
            [r, g, bl] => b.fill_rgb(*r, *g, *bl),
            _ => b.fill_gray(0.0),
        };
    }
    pub(crate) fn set_stroke(&self, b: &mut ContentBuilder) {
        match self.0.as_slice() {
            [g] => b.stroke_gray(*g),
            [c, m, y, k] => b.stroke_cmyk(*c, *m, *y, *k),
            [r, g, bl] => b.stroke_rgb(*r, *g, *bl),
            _ => b.stroke_gray(0.0),
        };
    }
}

/// User-settable properties. `None` means "use the type's default" on creation and "leave as is"
/// in a patch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnnotProps {
    /// `/C`: markup colour, stroke colour, icon colour, or (FreeText) border colour.
    pub color: Option<Color>,
    /// `/CA` constant opacity, 0..=1.
    pub opacity: Option<f64>,
    /// `/BS /W` line or border width in points.
    pub width: Option<f64>,
    /// `/BS /D` dash array; empty is solid.
    pub dash: Vec<f64>,
    /// `/IC` interior colour (shapes, line endings, FreeText background).
    pub fill: Option<Color>,
    /// `/T` author.
    pub author: Option<String>,
    /// `/Subj`.
    pub subject: Option<String>,
    /// `/Contents` (for a text box this is the text itself).
    pub contents: Option<String>,
    /// `/CreationDate`, a PDF date string; filled with the current time when absent.
    pub created: Option<String>,
    /// `/M`, a PDF date string; filled with the current time when absent.
    pub modified: Option<String>,
    /// `/F`; defaults to Print (sticky notes and replies also NoZoom|NoRotate).
    pub flags: Option<u32>,
    /// `/NM`, the unique annotation name; generated when absent.
    pub name: Option<String>,
}

impl AnnotProps {
    pub fn with_color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn with_opacity(mut self, o: f64) -> Self {
        self.opacity = Some(o);
        self
    }
    pub fn with_width(mut self, w: f64) -> Self {
        self.width = Some(w);
        self
    }
    pub fn with_fill(mut self, c: Color) -> Self {
        self.fill = Some(c);
        self
    }
    pub fn with_author(mut self, a: impl Into<String>) -> Self {
        self.author = Some(a.into());
        self
    }
    pub fn with_subject(mut self, s: impl Into<String>) -> Self {
        self.subject = Some(s.into());
        self
    }
    pub fn with_contents(mut self, s: impl Into<String>) -> Self {
        self.contents = Some(s.into());
        self
    }
    pub fn with_name(mut self, s: impl Into<String>) -> Self {
        self.name = Some(s.into());
        self
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(c) = &self.color {
            c.validate()?;
        }
        if let Some(c) = &self.fill {
            c.validate()?;
        }
        if let Some(o) = self.opacity
            && !(0.0..=1.0).contains(&o)
        {
            return Err(Error::invalid("opacity must be within 0..=1"));
        }
        if let Some(w) = self.width
            && !(w.is_finite() && (0.0..=200.0).contains(&w))
        {
            return Err(Error::invalid("width must be within 0..=200 points"));
        }
        if self
            .dash
            .iter()
            .any(|d| !d.is_finite() || *d < 0.0 || *d > 1000.0)
        {
            return Err(Error::invalid("invalid dash array"));
        }
        for d in [&self.created, &self.modified].into_iter().flatten() {
            if !d.starts_with("D:") {
                return Err(Error::invalid("dates must be PDF date strings (D:YYYY...)"));
            }
        }
        Ok(())
    }
}

/// Keys a patch can clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKey {
    Fill,
    Subject,
    Contents,
    Dash,
    Author,
}

/// A partial update of [`AnnotProps`]: set fields override, `clear` removes keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PropsPatch {
    pub set: AnnotProps,
    pub clear: Vec<PropKey>,
}

impl PropsPatch {
    pub fn new(set: AnnotProps) -> Self {
        PropsPatch {
            set,
            clear: Vec::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.set == AnnotProps::default() && self.clear.is_empty()
    }
    /// Apply to `base`; `name` and `created` are never changed by a patch.
    pub fn apply_to(&self, base: &mut AnnotProps) {
        let s = &self.set;
        macro_rules! over {
            ($($f:ident),*) => { $( if s.$f.is_some() { base.$f = s.$f.clone(); } )* };
        }
        over!(
            color, opacity, width, fill, author, subject, contents, modified, flags
        );
        if !s.dash.is_empty() {
            base.dash = s.dash.clone();
        }
        for k in &self.clear {
            match k {
                PropKey::Fill => base.fill = None,
                PropKey::Subject => base.subject = None,
                PropKey::Contents => base.contents = None,
                PropKey::Dash => base.dash.clear(),
                PropKey::Author => base.author = None,
            }
        }
    }
}

/// Current UTC time as a PDF date string `D:YYYYMMDDHHmmSSZ`.
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    pdf_date_from_unix(secs)
}

pub fn pdf_date_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A unique `/NM` value: 128 random-looking bits from the clock, a counter and the process id.
pub fn new_annotation_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = N.fetch_add(1, Ordering::Relaxed);
    let mut x = nanos ^ c.rotate_left(32) ^ (u64::from(std::process::id()) << 16);
    // splitmix64 twice for two words
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let (a, b) = (next(), next());
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16,
        (b >> 48) as u16,
        b & 0xFFFF_FFFF_FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(pdf_date_from_unix(0), "D:19700101000000Z");
        assert_eq!(pdf_date_from_unix(1_791_000_000), "D:20261003040000Z");
        assert_eq!(pdf_date_from_unix(951_782_400), "D:20000229000000Z");
    }

    #[test]
    fn names_are_unique() {
        let a = new_annotation_name();
        let b = new_annotation_name();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }

    #[test]
    fn patch_applies_and_clears() {
        let mut p = AnnotProps::default()
            .with_fill(Color::gray(0.5))
            .with_subject("s");
        let patch = PropsPatch {
            set: AnnotProps::default().with_opacity(0.5),
            clear: vec![PropKey::Fill, PropKey::Subject],
        };
        patch.apply_to(&mut p);
        assert_eq!(p.opacity, Some(0.5));
        assert!(p.fill.is_none() && p.subject.is_none());
    }
}
