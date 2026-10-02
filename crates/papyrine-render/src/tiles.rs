//! Pure tile math: √2 zoom buckets, page grids, viewport coverage.
//!
//! `scale` is device pixels per PDF point (device-pixel-ratio included), so
//! scale 1.0 is 72 dpi. Bucket `b` renders at `scale = √2^b`; the UI GPU-scales
//! between buckets.

pub const TILE_SIZE: u32 = 512;
pub const MIN_BUCKET: i32 = -8;
pub const MAX_BUCKET: i32 = 16;
/// Largest page dimension in pixels we allow at any bucket.
pub const MAX_PAGE_PX: u32 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    /// Closest bucket in log space (sharp enough, cheapest).
    Nearest,
    /// Smallest bucket with scale >= requested (never upscaled).
    Up,
}

pub fn bucket_scale(bucket: i32) -> f64 {
    std::f64::consts::SQRT_2.powi(bucket)
}

pub fn bucket_for_scale(scale: f64, rounding: Rounding) -> i32 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0;
    }
    let x = scale.log2() * 2.0;
    let b = match rounding {
        Rounding::Nearest => x.round(),
        Rounding::Up => (x - 1e-9).ceil(),
    };
    (b as i32).clamp(MIN_BUCKET, MAX_BUCKET)
}

/// Highest bucket at which the page still fits in `MAX_PAGE_PX`.
pub fn clamp_bucket_for_page(bucket: i32, w_pts: f32, h_pts: f32) -> i32 {
    let mut b = bucket.clamp(MIN_BUCKET, MAX_BUCKET);
    let longest = w_pts.max(h_pts).max(1.0) as f64;
    while b > MIN_BUCKET && longest * bucket_scale(b) > MAX_PAGE_PX as f64 {
        b -= 1;
    }
    b
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    pub col: u32,
    pub row: u32,
}

/// Pixel-space geometry of one page at one bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageGrid {
    pub width_px: u32,
    pub height_px: u32,
    pub cols: u32,
    pub rows: u32,
}

/// Axis-aligned rectangle in bucket-pixel space (origin top-left of the page).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectPx {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Integer tile rectangle clipped to the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub fn page_grid(w_pts: f32, h_pts: f32, bucket: i32) -> PageGrid {
    let s = bucket_scale(bucket);
    let w = ((w_pts as f64 * s).round() as u32).clamp(1, MAX_PAGE_PX);
    let h = ((h_pts as f64 * s).round() as u32).clamp(1, MAX_PAGE_PX);
    PageGrid {
        width_px: w,
        height_px: h,
        cols: w.div_ceil(TILE_SIZE),
        rows: h.div_ceil(TILE_SIZE),
    }
}

pub fn tile_rect(grid: &PageGrid, c: TileCoord) -> Option<TileRect> {
    if c.col >= grid.cols || c.row >= grid.rows {
        return None;
    }
    let x = c.col * TILE_SIZE;
    let y = c.row * TILE_SIZE;
    Some(TileRect {
        x,
        y,
        w: TILE_SIZE.min(grid.width_px - x),
        h: TILE_SIZE.min(grid.height_px - y),
    })
}

/// Tiles intersecting `viewport`, expanded by `margin_tiles` (prefetch ring),
/// ordered visible tiles first, then the ring, each nearest-to-centre first
/// (render priority).
pub fn tiles_covering(grid: &PageGrid, viewport: RectPx, margin_tiles: u32) -> Vec<TileCoord> {
    if grid.cols == 0 || grid.rows == 0 || viewport.w <= 0.0 || viewport.h <= 0.0 {
        return Vec::new();
    }
    let t = TILE_SIZE as f64;
    // Visible tile index range on one axis, clamped to the page.
    let range = |lo: f64, len: f64, count: u32| -> Option<(u32, u32)> {
        let first = (lo / t).floor();
        let last = ((lo + len) / t).ceil() - 1.0;
        if last < 0.0 || first > (count - 1) as f64 {
            return None;
        }
        Some((first.max(0.0) as u32, (last as u32).min(count - 1)))
    };
    let (Some((vc0, vc1)), Some((vr0, vr1))) = (
        range(viewport.x, viewport.w, grid.cols),
        range(viewport.y, viewport.h, grid.rows),
    ) else {
        return Vec::new();
    };
    let (c0, c1) = (
        vc0.saturating_sub(margin_tiles),
        (vc1 + margin_tiles).min(grid.cols - 1),
    );
    let (r0, r1) = (
        vr0.saturating_sub(margin_tiles),
        (vr1 + margin_tiles).min(grid.rows - 1),
    );
    let cx = (viewport.x + viewport.w / 2.0) / t - 0.5;
    let cy = (viewport.y + viewport.h / 2.0) / t - 0.5;
    let mut v: Vec<TileCoord> = (r0..=r1)
        .flat_map(|row| (c0..=c1).map(move |col| TileCoord { col, row }))
        .collect();
    let ring = |c: &TileCoord| !((vc0..=vc1).contains(&c.col) && (vr0..=vr1).contains(&c.row));
    let dist = |c: &TileCoord| (c.col as f64 - cx).powi(2) + (c.row as f64 - cy).powi(2);
    v.sort_by(|a, b| {
        ring(a)
            .cmp(&ring(b))
            .then(dist(a).total_cmp(&dist(b)))
            .then(a.cmp(b))
    });
    v
}

/// Convert a rectangle in page points (top-left origin, y down) to bucket pixels.
pub fn pts_to_px(r: RectPx, bucket: i32) -> RectPx {
    let s = bucket_scale(bucket);
    RectPx {
        x: r.x * s,
        y: r.y * s,
        w: r.w * s,
        h: r.h * s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        assert_eq!(bucket_for_scale(1.0, Rounding::Nearest), 0);
        assert_eq!(bucket_for_scale(2.0, Rounding::Nearest), 2);
        assert_eq!(bucket_for_scale(std::f64::consts::SQRT_2, Rounding::Up), 1);
        assert_eq!(bucket_for_scale(1.01, Rounding::Up), 1);
        assert_eq!(bucket_for_scale(1.01, Rounding::Nearest), 0);
        assert_eq!(bucket_for_scale(1.18, Rounding::Nearest), 0);
        assert_eq!(bucket_for_scale(1.2, Rounding::Nearest), 1);
        assert_eq!(bucket_for_scale(0.5, Rounding::Up), -2);
        assert_eq!(bucket_for_scale(1e9, Rounding::Up), MAX_BUCKET);
        assert_eq!(bucket_for_scale(f64::NAN, Rounding::Up), 0);
        assert!((bucket_scale(2) - 2.0).abs() < 1e-12);
        assert!((bucket_scale(-2) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn clamp_big_pages() {
        // 5 m x 5 m drawing = ~14173 pt
        let b = clamp_bucket_for_page(MAX_BUCKET, 14173.0, 14173.0);
        assert!(14173.0 * bucket_scale(b) <= MAX_PAGE_PX as f64);
        assert!(14173.0 * bucket_scale(b + 1) > MAX_PAGE_PX as f64);
        assert_eq!(clamp_bucket_for_page(3, 612.0, 792.0), 3);
    }

    #[test]
    fn grid_and_rects() {
        let g = page_grid(612.0, 792.0, 0);
        assert_eq!((g.width_px, g.height_px, g.cols, g.rows), (612, 792, 2, 2));
        assert_eq!(
            tile_rect(&g, TileCoord { col: 1, row: 1 }),
            Some(TileRect {
                x: 512,
                y: 512,
                w: 100,
                h: 280
            })
        );
        assert_eq!(tile_rect(&g, TileCoord { col: 2, row: 0 }), None);
        let g2 = page_grid(612.0, 792.0, 2);
        assert_eq!(
            (g2.width_px, g2.height_px, g2.cols, g2.rows),
            (1224, 1584, 3, 4)
        );
        let exact = page_grid(512.0, 1024.0, 0);
        assert_eq!((exact.cols, exact.rows), (1, 2));
    }

    #[test]
    fn coverage() {
        let g = page_grid(612.0, 3000.0, 0); // 2 cols x 6 rows
        let vp = RectPx {
            x: 0.0,
            y: 600.0,
            w: 612.0,
            h: 500.0,
        }; // rows 1..=2
        let t = tiles_covering(&g, vp, 0);
        assert_eq!(t.len(), 4);
        assert!(t.iter().all(|c| (1..=2).contains(&c.row)));
        let t1 = tiles_covering(&g, vp, 1);
        assert_eq!(t1.len(), 2 * 4); // rows 0..=3
        // centre-first: all tiles of a ring come after the visible ones
        let visible: Vec<_> = t.iter().collect();
        assert!(t1[..4].iter().all(|c| visible.contains(&c)));
        // exact boundary: viewport ending at tile edge does not include next row
        let edge = RectPx {
            x: 0.0,
            y: 0.0,
            w: 512.0,
            h: 512.0,
        };
        assert_eq!(
            tiles_covering(&g, edge, 0),
            vec![TileCoord { col: 0, row: 0 }]
        );
        // outside and degenerate
        assert!(
            tiles_covering(
                &g,
                RectPx {
                    x: 0.0,
                    y: 9000.0,
                    w: 10.0,
                    h: 10.0
                },
                0
            )
            .is_empty()
        );
        assert!(
            tiles_covering(
                &g,
                RectPx {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 10.0
                },
                0
            )
            .is_empty()
        );
        // partially off-page viewport
        let neg = RectPx {
            x: -300.0,
            y: -300.0,
            w: 400.0,
            h: 400.0,
        };
        assert_eq!(
            tiles_covering(&g, neg, 0),
            vec![TileCoord { col: 0, row: 0 }]
        );
    }
}
