//! Geometry in PDF conventions: user space has the origin at the bottom-left
//! and y grows upward; device space has the origin at the top-left and y grows
//! downward. Matrices are the PDF `[a b c d e f]` form applied to row vectors:
//! `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

/// Axis-aligned rectangle as `[llx lly urx ury]`, the PDF array order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn from_pdf_array(a: [f64; 4]) -> Self {
        Self::new(a[0], a[1], a[2], a[3]).normalized()
    }

    pub fn to_pdf_array(self) -> [f64; 4] {
        [self.x0, self.y0, self.x1, self.y1]
    }

    /// PDF allows any two opposite corners; this puts them in (min, max) order.
    pub fn normalized(self) -> Self {
        Self::new(
            self.x0.min(self.x1),
            self.y0.min(self.y1),
            self.x0.max(self.x1),
            self.y0.max(self.y1),
        )
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }

    pub fn is_empty(&self) -> bool {
        !(self.width() > 0.0 && self.height() > 0.0)
    }

    pub fn center(&self) -> Point {
        Point::new((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    /// Edge-inclusive containment.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }

    pub fn contains_rect(&self, r: &Rect) -> bool {
        r.x0 >= self.x0 && r.x1 <= self.x1 && r.y0 >= self.y0 && r.y1 <= self.y1
    }

    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(
            self.x0.min(o.x0),
            self.y0.min(o.y0),
            self.x1.max(o.x1),
            self.y1.max(o.y1),
        )
    }

    /// `None` when the rectangles do not overlap (touching edges count as empty).
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect::new(
            self.x0.max(o.x0),
            self.y0.max(o.y0),
            self.x1.min(o.x1),
            self.y1.min(o.y1),
        );
        (r.x0 < r.x1 && r.y0 < r.y1).then_some(r)
    }

    pub fn inflate(&self, d: f64) -> Rect {
        Rect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }

    /// Bounding box of the four transformed corners.
    pub fn transform(&self, m: &Matrix) -> Rect {
        let pts = [
            m.apply(Point::new(self.x0, self.y0)),
            m.apply(Point::new(self.x1, self.y0)),
            m.apply(Point::new(self.x1, self.y1)),
            m.apply(Point::new(self.x0, self.y1)),
        ];
        let mut r = Rect::new(pts[0].x, pts[0].y, pts[0].x, pts[0].y);
        for p in &pts[1..] {
            r.x0 = r.x0.min(p.x);
            r.y0 = r.y0.min(p.y);
            r.x1 = r.x1.max(p.x);
            r.y1 = r.y1.max(p.y);
        }
        r
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn from_array(m: [f64; 6]) -> Self {
        Self::new(m[0], m[1], m[2], m[3], m[4], m[5])
    }

    pub fn to_array(self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    pub const fn translate(tx: f64, ty: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// Counter-clockwise rotation by `radians` in user space.
    pub fn rotate(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Self::new(c, s, -s, c, 0.0, 0.0)
    }

    /// `self` applied first, then `then`; PDF writes this `self x then`.
    /// (`cm` operands combine as `new_cm.multiply(&ctm)`.)
    pub fn multiply(&self, then: &Matrix) -> Matrix {
        Matrix::new(
            self.a * then.a + self.b * then.c,
            self.a * then.b + self.b * then.d,
            self.c * then.a + self.d * then.c,
            self.c * then.b + self.d * then.d,
            self.e * then.a + self.f * then.c + then.e,
            self.e * then.b + self.f * then.d + then.f,
        )
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// `None` for singular (or non-finite) matrices.
    pub fn invert(&self) -> Option<Matrix> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Matrix::new(
            self.d * inv,
            -self.b * inv,
            -self.c * inv,
            self.a * inv,
            (self.c * self.f - self.d * self.e) * inv,
            (self.b * self.e - self.a * self.f) * inv,
        ))
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// Transform a direction (ignores translation).
    pub fn apply_vector(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y, self.b * p.x + self.d * p.y)
    }

    /// Mean scale factor, `sqrt(|det|)`.
    pub fn mean_scale(&self) -> f64 {
        self.determinant().abs().sqrt()
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// User space to device space for a page box. `rotation` is the page's
    /// clockwise `/Rotate` in degrees (normalized to a multiple of 90);
    /// `scale` is device pixels per user-space point. Also returns the device
    /// size in pixels.
    pub fn page_to_device(boxr: Rect, rotation: i32, scale: f64) -> (Matrix, Size) {
        let r = boxr.normalized();
        let s = scale;
        let (w, h) = (r.width() * s, r.height() * s);
        match rotation.rem_euclid(360) / 90 {
            0 => (
                Matrix::new(s, 0.0, 0.0, -s, -r.x0 * s, r.y1 * s),
                Size::new(w, h),
            ),
            1 => (
                Matrix::new(0.0, s, s, 0.0, -r.y0 * s, -r.x0 * s),
                Size::new(h, w),
            ),
            2 => (
                Matrix::new(-s, 0.0, 0.0, s, r.x1 * s, -r.y0 * s),
                Size::new(w, h),
            ),
            _ => (
                Matrix::new(0.0, -s, -s, 0.0, r.y1 * s, r.x1 * s),
                Size::new(h, w),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn multiply_order_is_self_then_other() {
        let t = Matrix::translate(10.0, 0.0);
        let s = Matrix::scale(2.0, 2.0);
        // translate then scale: (1,0) -> (11,0) -> (22,0)
        assert!(near(
            t.multiply(&s).apply(Point::new(1.0, 0.0)),
            Point::new(22.0, 0.0)
        ));
        // scale then translate: (1,0) -> (2,0) -> (12,0)
        assert!(near(
            s.multiply(&t).apply(Point::new(1.0, 0.0)),
            Point::new(12.0, 0.0)
        ));
    }

    #[test]
    fn invert_roundtrip() {
        let m = Matrix::new(2.0, 1.0, -1.0, 3.0, 5.0, -7.0);
        let inv = m.invert().unwrap();
        let p = Point::new(3.5, -2.25);
        assert!(near(inv.apply(m.apply(p)), p));
        let id = m.multiply(&inv);
        for (x, y) in id.to_array().iter().zip(Matrix::IDENTITY.to_array()) {
            assert!((x - y).abs() < 1e-12);
        }
        assert!(Matrix::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn rotate_quarter_turn() {
        let r = Matrix::rotate(std::f64::consts::FRAC_PI_2);
        assert!(near(r.apply(Point::new(1.0, 0.0)), Point::new(0.0, 1.0)));
    }

    #[test]
    fn rect_ops() {
        let r = Rect::from_pdf_array([10.0, 20.0, 0.0, 5.0]);
        assert_eq!(r, Rect::new(0.0, 5.0, 10.0, 20.0));
        assert_eq!((r.width(), r.height()), (10.0, 15.0));
        assert!(r.contains(Point::new(10.0, 20.0)));
        assert!(!r.contains(Point::new(10.1, 20.0)));
        let o = Rect::new(5.0, 10.0, 30.0, 30.0);
        assert_eq!(r.intersect(&o), Some(Rect::new(5.0, 10.0, 10.0, 20.0)));
        assert_eq!(r.union(&o), Rect::new(0.0, 5.0, 30.0, 30.0));
        assert_eq!(r.intersect(&Rect::new(10.0, 0.0, 20.0, 50.0)), None);
        assert!(Rect::new(0.0, 0.0, 0.0, 5.0).is_empty());
    }

    #[test]
    fn rect_transform_bounds() {
        let r = Rect::new(0.0, 0.0, 2.0, 1.0);
        let t = r.transform(&Matrix::rotate(std::f64::consts::FRAC_PI_2));
        assert!((t.x0 + 1.0).abs() < 1e-9 && t.x1.abs() < 1e-9);
        assert!(t.y0.abs() < 1e-9 && (t.y1 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn page_to_device_all_rotations() {
        // 200 x 100 page at (10, 20), scale 2.
        let b = Rect::new(10.0, 20.0, 210.0, 120.0);
        let ll = Point::new(10.0, 20.0); // bottom-left
        let ul = Point::new(10.0, 120.0); // top-left
        let lr = Point::new(210.0, 20.0); // bottom-right

        let (m, s) = Matrix::page_to_device(b, 0, 2.0);
        assert_eq!(s, Size::new(400.0, 200.0));
        assert!(near(m.apply(ul), Point::new(0.0, 0.0)));
        assert!(near(m.apply(ll), Point::new(0.0, 200.0)));

        let (m, s) = Matrix::page_to_device(b, 90, 2.0);
        assert_eq!(s, Size::new(200.0, 400.0));
        assert!(near(m.apply(ll), Point::new(0.0, 0.0)));
        assert!(near(m.apply(ul), Point::new(200.0, 0.0)));
        assert!(near(m.apply(lr), Point::new(0.0, 400.0)));

        let (m, s) = Matrix::page_to_device(b, 180, 2.0);
        assert_eq!(s, Size::new(400.0, 200.0));
        assert!(near(m.apply(ll), Point::new(400.0, 0.0)));

        let (m, s) = Matrix::page_to_device(b, -90, 2.0); // == 270
        assert_eq!(s, Size::new(200.0, 400.0));
        assert!(near(m.apply(ll), Point::new(200.0, 400.0)));
        assert!(near(m.apply(ul), Point::new(0.0, 400.0)));
        assert!(near(m.apply(lr), Point::new(200.0, 0.0)));
    }
}
