//! Page placement math for printing: fit, shrink oversized, actual size, auto-rotate and
//! centering. Pure functions over geometry; no PDF objects (ARCHITECTURE section 11.2).
//!
//! Coordinates are PDF user space: points, origin bottom-left, y up.

use papyrine_core::{Matrix, Point, Rect, Size};
use serde::{Deserialize, Serialize};

/// How a page is scaled onto the printable area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// Scale up or down to fill the printable area (aspect preserved).
    Fit,
    /// Scale down only when the page is larger than the printable area.
    #[default]
    ShrinkOversized,
    /// 100 %; oversized pages are clipped by the device.
    ActualSize,
}

/// Placement options shared by every back end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub scaling: Scaling,
    /// Turn the page a quarter turn counter-clockwise when its orientation differs from the
    /// paper's (the direction PDFKit and CUPS use).
    pub auto_rotate: bool,
    /// Centre the page on the printable area (otherwise top-left aligned).
    pub center: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            scaling: Scaling::ShrinkOversized,
            auto_rotate: true,
            center: true,
        }
    }
}

/// Paper and its non-printable margins, in points, for the orientation being printed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub width: f64,
    pub height: f64,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
}

impl Sheet {
    /// US Letter, no margins.
    pub const LETTER: Sheet = Sheet::new(612.0, 792.0);

    pub const fn new(width: f64, height: f64) -> Sheet {
        Sheet {
            width,
            height,
            margin_top: 0.0,
            margin_right: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,
        }
    }

    pub const fn with_margins(mut self, top: f64, right: f64, bottom: f64, left: f64) -> Sheet {
        self.margin_top = top;
        self.margin_right = right;
        self.margin_bottom = bottom;
        self.margin_left = left;
        self
    }

    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    /// The imageable area (never negative-sized).
    pub fn printable_area(&self) -> Rect {
        let x0 = self.margin_left.min(self.width);
        let y0 = self.margin_bottom.min(self.height);
        let x1 = (self.width - self.margin_right).max(x0);
        let y1 = (self.height - self.margin_top).max(y0);
        Rect::new(x0, y0, x1, y1)
    }
}

/// Where one page lands on the sheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Maps the page's unrotated user space to sheet space (`x' = a x + c y + e`, ...).
    pub matrix: Matrix,
    /// Clockwise quarter turns applied in total (the page's `/Rotate` plus any auto-rotation).
    pub quarter_turns: u8,
    /// True when auto-rotate added a turn on top of `/Rotate`.
    pub auto_rotated: bool,
    pub scale: f64,
    /// The page box as placed on the sheet.
    pub placed: Rect,
}

/// Quarter turns clockwise for a `/Rotate` value (any integer; wraps).
fn turns_of(rotate: i32) -> u8 {
    (rotate.div_euclid(90).rem_euclid(4)) as u8
}

/// Unrotated box to an origin-based, y-up rectangle after `turns` clockwise quarter turns.
fn base_matrix(b: Rect, turns: u8) -> (Matrix, Size) {
    let (w, h) = (b.width(), b.height());
    match turns % 4 {
        0 => (Matrix::translate(-b.x0, -b.y0), Size::new(w, h)),
        1 => (
            Matrix::new(0.0, -1.0, 1.0, 0.0, -b.y0, b.x1),
            Size::new(h, w),
        ),
        2 => (
            Matrix::new(-1.0, 0.0, 0.0, -1.0, b.x1, b.y1),
            Size::new(w, h),
        ),
        _ => (
            Matrix::new(0.0, 1.0, -1.0, 0.0, b.y1, -b.x0),
            Size::new(h, w),
        ),
    }
}

/// Place a page (`page_box` is its visible box, e.g. the crop box; `page_rotate` its `/Rotate`)
/// on the printable `area` of a sheet.
pub fn place_page(page_box: Rect, page_rotate: i32, area: Rect, layout: &Layout) -> Placement {
    let b = page_box.normalized();
    let base = turns_of(page_rotate);
    let (aw, ah) = (area.width(), area.height());
    // The page as displayed (after /Rotate) decides whether auto-rotate has anything to do.
    let displayed = if base.is_multiple_of(2) {
        Size::new(b.width(), b.height())
    } else {
        Size::new(b.height(), b.width())
    };
    let page_landscape = displayed.w > displayed.h;
    let area_landscape = aw > ah;
    let square = (displayed.w - displayed.h).abs() < 1e-6 || (aw - ah).abs() < 1e-6;
    let auto_rotated = layout.auto_rotate && !square && page_landscape != area_landscape;
    // Counter-clockwise, like PDFKit and CUPS do for landscape pages on portrait paper.
    let turns = (base + 3 * u8::from(auto_rotated)) % 4;

    let (base_m, size) = base_matrix(b, turns);
    let fit = if size.w > 0.0 && size.h > 0.0 {
        (aw / size.w).min(ah / size.h)
    } else {
        1.0
    };
    let scale = match layout.scaling {
        Scaling::Fit => fit,
        Scaling::ShrinkOversized => fit.min(1.0),
        Scaling::ActualSize => 1.0,
    };
    let (pw, ph) = (size.w * scale, size.h * scale);
    let (ox, oy) = if layout.center {
        (area.x0 + (aw - pw) / 2.0, area.y0 + (ah - ph) / 2.0)
    } else {
        // Top-left of the printable area.
        (area.x0, area.y1 - ph)
    };
    let matrix = base_m
        .multiply(&Matrix::scale(scale, scale))
        .multiply(&Matrix::translate(ox, oy));
    Placement {
        matrix,
        quarter_turns: turns,
        auto_rotated,
        scale,
        placed: Rect::new(ox, oy, ox + pw, oy + ph),
    }
}

/// Where a page box corner lands (test and diagnostics helper).
pub fn map_point(m: &Matrix, x: f64, y: f64) -> Point {
    m.apply(Point::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: Sheet = Sheet::LETTER;

    fn close(a: Point, x: f64, y: f64) -> bool {
        (a.x - x).abs() < 1e-6 && (a.y - y).abs() < 1e-6
    }

    fn lay(scaling: Scaling) -> Layout {
        Layout {
            scaling,
            auto_rotate: false,
            center: true,
        }
    }

    #[test]
    fn identity_when_page_matches_paper() {
        let p = place_page(
            Rect::new(0.0, 0.0, 612.0, 792.0),
            0,
            LETTER.printable_area(),
            &lay(Scaling::Fit),
        );
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.quarter_turns, 0);
        assert!(close(map_point(&p.matrix, 0.0, 0.0), 0.0, 0.0));
        assert!(close(map_point(&p.matrix, 612.0, 792.0), 612.0, 792.0));
    }

    #[test]
    fn fit_scales_up_and_centres() {
        // 306x396 is exactly half of letter: Fit doubles it; a 200x396 page is width-limited
        // by height and ends up centred horizontally.
        let p = place_page(
            Rect::new(0.0, 0.0, 306.0, 396.0),
            0,
            LETTER.printable_area(),
            &lay(Scaling::Fit),
        );
        assert_eq!(p.scale, 2.0);
        let q = place_page(
            Rect::new(0.0, 0.0, 200.0, 396.0),
            0,
            LETTER.printable_area(),
            &lay(Scaling::Fit),
        );
        assert_eq!(q.scale, 2.0);
        assert!(close(
            map_point(&q.matrix, 0.0, 0.0),
            (612.0 - 400.0) / 2.0,
            0.0
        ));
        assert_eq!(q.placed, Rect::new(106.0, 0.0, 506.0, 792.0));
    }

    #[test]
    fn shrink_oversized_only_shrinks() {
        let area = LETTER.printable_area();
        let big = place_page(
            Rect::new(0.0, 0.0, 1224.0, 1584.0),
            0,
            area,
            &lay(Scaling::ShrinkOversized),
        );
        assert_eq!(big.scale, 0.5);
        let small = place_page(
            Rect::new(0.0, 0.0, 300.0, 400.0),
            0,
            area,
            &lay(Scaling::ShrinkOversized),
        );
        assert_eq!(small.scale, 1.0);
        // Centred: (612-300)/2, (792-400)/2.
        assert!(close(map_point(&small.matrix, 0.0, 0.0), 156.0, 196.0));
    }

    #[test]
    fn actual_size_keeps_scale_and_overflows_symmetrically() {
        let p = place_page(
            Rect::new(0.0, 0.0, 700.0, 900.0),
            0,
            LETTER.printable_area(),
            &lay(Scaling::ActualSize),
        );
        assert_eq!(p.scale, 1.0);
        assert!(close(map_point(&p.matrix, 0.0, 0.0), -44.0, -54.0));
    }

    #[test]
    fn margins_shrink_the_area() {
        let sheet = Sheet::new(612.0, 792.0).with_margins(36.0, 18.0, 36.0, 18.0);
        let area = sheet.printable_area();
        assert_eq!(area, Rect::new(18.0, 36.0, 594.0, 756.0));
        let p = place_page(
            Rect::new(0.0, 0.0, 612.0, 792.0),
            0,
            area,
            &lay(Scaling::ShrinkOversized),
        );
        let s = (576.0f64 / 612.0).min(720.0 / 792.0);
        assert!((p.scale - s).abs() < 1e-12);
        assert!(p.placed.x0 >= 18.0 - 1e-9 && p.placed.x1 <= 594.0 + 1e-9);
        assert!(p.placed.y0 >= 36.0 - 1e-9 && p.placed.y1 <= 756.0 + 1e-9);
    }

    #[test]
    fn not_centred_aligns_top_left() {
        let layout = Layout {
            center: false,
            ..lay(Scaling::ActualSize)
        };
        let p = place_page(
            Rect::new(0.0, 0.0, 300.0, 400.0),
            0,
            LETTER.printable_area(),
            &layout,
        );
        // Page top-left (0,400) lands on the area's top-left (0,792).
        assert!(close(map_point(&p.matrix, 0.0, 400.0), 0.0, 792.0));
    }

    #[test]
    fn auto_rotate_turns_landscape_onto_portrait_paper() {
        let layout = Layout {
            auto_rotate: true,
            ..lay(Scaling::ShrinkOversized)
        };
        let b = Rect::new(0.0, 0.0, 792.0, 612.0);
        let p = place_page(b, 0, LETTER.printable_area(), &layout);
        assert!(p.auto_rotated);
        assert_eq!(p.quarter_turns, 3);
        assert_eq!(p.scale, 1.0);
        // 90 counter-clockwise: bottom-left goes to bottom-right, top-left to bottom-left.
        assert!(close(map_point(&p.matrix, 0.0, 0.0), 612.0, 0.0));
        assert!(close(map_point(&p.matrix, 0.0, 612.0), 0.0, 0.0));
        assert!(close(map_point(&p.matrix, 792.0, 0.0), 612.0, 792.0));
        // Without auto-rotate it shrinks instead.
        let q = place_page(
            b,
            0,
            LETTER.printable_area(),
            &lay(Scaling::ShrinkOversized),
        );
        assert!(!q.auto_rotated);
        assert!((q.scale - 612.0 / 792.0).abs() < 1e-12);
    }

    #[test]
    fn auto_rotate_respects_page_rotate_and_squares() {
        let layout = Layout {
            auto_rotate: true,
            ..lay(Scaling::Fit)
        };
        // A portrait box with /Rotate 90 displays as landscape: needs one more turn.
        let p = place_page(
            Rect::new(0.0, 0.0, 612.0, 792.0),
            90,
            LETTER.printable_area(),
            &layout,
        );
        assert!(p.auto_rotated);
        assert_eq!(p.quarter_turns, 0);
        // Squares never rotate.
        let sq = place_page(
            Rect::new(0.0, 0.0, 500.0, 500.0),
            0,
            LETTER.printable_area(),
            &layout,
        );
        assert!(!sq.auto_rotated);
        // Already matching orientation: untouched.
        let ok = place_page(
            Rect::new(0.0, 0.0, 612.0, 792.0),
            0,
            LETTER.printable_area(),
            &layout,
        );
        assert!(!ok.auto_rotated);
    }

    #[test]
    fn all_rotations_map_the_box_onto_the_origin_rect() {
        let b = Rect::new(10.0, 20.0, 110.0, 320.0); // 100 x 300, offset origin
        for (rot, w, h) in [
            (0, 100.0, 300.0),
            (90, 300.0, 100.0),
            (180, 100.0, 300.0),
            (270, 300.0, 100.0),
        ] {
            let (m, size) = base_matrix(b, turns_of(rot));
            assert_eq!((size.w, size.h), (w, h), "rot {rot}");
            let r = Rect::new(b.x0, b.y0, b.x1, b.y1).transform(&m);
            assert!(
                (r.x0).abs() < 1e-9 && (r.y0).abs() < 1e-9,
                "rot {rot}: {r:?}"
            );
            assert!(
                (r.x1 - w).abs() < 1e-9 && (r.y1 - h).abs() < 1e-9,
                "rot {rot}: {r:?}"
            );
        }
        // Rotate 180: bottom-left of the box lands on the top-right.
        let (m, _) = base_matrix(b, 2);
        assert!(close(map_point(&m, 10.0, 20.0), 100.0, 300.0));
        // Rotate 270 clockwise: bottom-left goes to bottom-right.
        let (m, _) = base_matrix(b, 3);
        assert!(close(map_point(&m, 10.0, 20.0), 300.0, 0.0));
    }

    #[test]
    fn rotate_values_wrap() {
        assert_eq!(turns_of(-90), 3);
        assert_eq!(turns_of(450), 1);
        assert_eq!(turns_of(0), 0);
    }

    #[test]
    fn degenerate_boxes_do_not_panic() {
        let p = place_page(
            Rect::new(0.0, 0.0, 0.0, 0.0),
            0,
            LETTER.printable_area(),
            &Layout::default(),
        );
        assert!(p.scale.is_finite());
        let tiny = Sheet::new(10.0, 10.0).with_margins(20.0, 20.0, 20.0, 20.0);
        assert!(tiny.printable_area().width() >= 0.0);
    }
}
