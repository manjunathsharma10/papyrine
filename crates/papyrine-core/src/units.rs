//! Units and page-size presets. PDF user space is 1/72 inch ("points").

pub const PT_PER_INCH: f64 = 72.0;
pub const MM_PER_INCH: f64 = 25.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Pt,
    Mm,
    In,
}

impl Unit {
    fn per_inch(self) -> f64 {
        match self {
            Unit::Pt => PT_PER_INCH,
            Unit::Mm => MM_PER_INCH,
            Unit::In => 1.0,
        }
    }

    pub fn convert(value: f64, from: Unit, to: Unit) -> f64 {
        value / from.per_inch() * to.per_inch()
    }

    pub fn to_pt(self, value: f64) -> f64 {
        Self::convert(value, self, Unit::Pt)
    }

    pub fn from_pt(self, pt: f64) -> f64 {
        Self::convert(pt, Unit::Pt, self)
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Pt => "pt",
            Unit::Mm => "mm",
            Unit::In => "in",
        }
    }
}

pub fn mm_to_pt(mm: f64) -> f64 {
    Unit::Mm.to_pt(mm)
}
pub fn in_to_pt(inches: f64) -> f64 {
    Unit::In.to_pt(inches)
}
pub fn pt_to_mm(pt: f64) -> f64 {
    Unit::Mm.from_pt(pt)
}
pub fn pt_to_in(pt: f64) -> f64 {
    Unit::In.from_pt(pt)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PageSize {
    A3,
    A4,
    A5,
    A6,
    B4,
    B5,
    Letter,
    Legal,
    Tabloid,
    Executive,
}

impl PageSize {
    pub const ALL: [PageSize; 10] = [
        PageSize::A3,
        PageSize::A4,
        PageSize::A5,
        PageSize::A6,
        PageSize::B4,
        PageSize::B5,
        PageSize::Letter,
        PageSize::Legal,
        PageSize::Tabloid,
        PageSize::Executive,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PageSize::A3 => "A3",
            PageSize::A4 => "A4",
            PageSize::A5 => "A5",
            PageSize::A6 => "A6",
            PageSize::B4 => "B4",
            PageSize::B5 => "B5",
            PageSize::Letter => "Letter",
            PageSize::Legal => "Legal",
            PageSize::Tabloid => "Tabloid",
            PageSize::Executive => "Executive",
        }
    }

    pub fn from_name(name: &str) -> Option<PageSize> {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Portrait (width, height) in points.
    pub fn portrait_pt(self) -> (f64, f64) {
        let mm = |w: f64, h: f64| (mm_to_pt(w), mm_to_pt(h));
        let inch = |w: f64, h: f64| (in_to_pt(w), in_to_pt(h));
        match self {
            PageSize::A3 => mm(297.0, 420.0),
            PageSize::A4 => mm(210.0, 297.0),
            PageSize::A5 => mm(148.0, 210.0),
            PageSize::A6 => mm(105.0, 148.0),
            PageSize::B4 => mm(250.0, 353.0),
            PageSize::B5 => mm(176.0, 250.0),
            PageSize::Letter => inch(8.5, 11.0),
            PageSize::Legal => inch(8.5, 14.0),
            PageSize::Tabloid => inch(11.0, 17.0),
            PageSize::Executive => inch(7.25, 10.5),
        }
    }

    pub fn landscape_pt(self) -> (f64, f64) {
        let (w, h) = self.portrait_pt();
        (h, w)
    }

    /// Nearest preset (either orientation) within `tolerance_pt` on both sides.
    pub fn detect(width_pt: f64, height_pt: f64, tolerance_pt: f64) -> Option<PageSize> {
        let (w, h) = if width_pt <= height_pt {
            (width_pt, height_pt)
        } else {
            (height_pt, width_pt)
        };
        Self::ALL
            .into_iter()
            .map(|p| {
                let (pw, ph) = p.portrait_pt();
                (p, (pw - w).abs().max((ph - h).abs()))
            })
            .filter(|(_, d)| *d <= tolerance_pt)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn conversions() {
        assert!(close(in_to_pt(1.0), 72.0));
        assert!(close(mm_to_pt(25.4), 72.0));
        assert!(close(pt_to_mm(72.0), 25.4));
        assert!(close(Unit::convert(10.0, Unit::Mm, Unit::In), 10.0 / 25.4));
        assert!(close(Unit::Pt.from_pt(Unit::Pt.to_pt(3.3)), 3.3));
    }

    #[test]
    fn presets() {
        let (w, h) = PageSize::A4.portrait_pt();
        assert!((w - 595.276).abs() < 0.01 && (h - 841.89).abs() < 0.01);
        assert_eq!(PageSize::Letter.portrait_pt(), (612.0, 792.0));
        assert_eq!(PageSize::Legal.landscape_pt(), (1008.0, 612.0));
        assert_eq!(PageSize::from_name(" letter "), Some(PageSize::Letter));
        assert_eq!(PageSize::from_name("nope"), None);
    }

    #[test]
    fn detection() {
        assert_eq!(PageSize::detect(595.0, 842.0, 2.0), Some(PageSize::A4));
        assert_eq!(PageSize::detect(792.0, 612.0, 1.0), Some(PageSize::Letter));
        assert_eq!(PageSize::detect(500.0, 500.0, 2.0), None);
    }
}
