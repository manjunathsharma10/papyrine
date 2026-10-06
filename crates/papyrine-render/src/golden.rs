//! Golden-image support: an exact hash plus a coarse perceptual signature per
//! rendered page, so the repository stores a few hundred bytes per page and no
//! images (total well under 1 MB for 50 pages).
//!
//! * The **hash** (FNV-1a 64 over the RGBA bytes) must match exactly on the
//!   platform and PDFium build it was recorded on.
//! * The **signature** has two planes over a `GRID x GRID` grid: the mean RGB
//!   of each cell and its ink coverage (share of non-white pixels). Comparing
//!   signatures tolerates anti-aliasing and font-rasteriser differences
//!   between operating systems but catches missing content, shifted layout,
//!   wrong colours and a large change in total ink (a missing form field).

use crate::Tile;

pub const GRID: usize = 16;
/// Longest edge, in pixels, of the golden page renders.
pub const GOLDEN_EDGE: u32 = 768;
/// Perceptual tolerance: mean absolute cell difference (0..=255) ...
pub const TOL_MEAN: f64 = 1.5;
/// ... and the largest single cell difference.
pub const TOL_MAX: u8 = 16;

pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Largest allowed change of a cell's ink coverage (0..=255) ...
pub const TOL_COV_MAX: u8 = 64;
/// ... and of the page's total ink, as a fraction of the larger total.
pub const TOL_INK: f64 = 0.08;

/// Mean RGB of each grid cell (row-major, `GRID * GRID * 3` bytes) followed by
/// each cell's ink coverage (`GRID * GRID` bytes).
pub fn signature(t: &Tile) -> Vec<u8> {
    let (w, h) = (t.width as usize, t.height as usize);
    let mut out = Vec::with_capacity(GRID * GRID * 4);
    let mut cov = Vec::with_capacity(GRID * GRID);
    for gy in 0..GRID {
        let (y0, y1) = (
            gy * h / GRID,
            ((gy + 1) * h / GRID).max(gy * h / GRID + 1).min(h),
        );
        for gx in 0..GRID {
            let (x0, x1) = (
                gx * w / GRID,
                ((gx + 1) * w / GRID).max(gx * w / GRID + 1).min(w),
            );
            let mut sum = [0u64; 3];
            let mut ink = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    for (acc, &v) in sum.iter_mut().zip(&t.rgba[i..i + 3]) {
                        *acc += u64::from(v);
                    }
                    ink += u64::from(t.rgba[i..i + 3].iter().any(|&v| v < 250));
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u64;
            for s in sum {
                out.push((s / n) as u8);
            }
            cov.push((ink * 255 / n) as u8);
        }
    }
    out.extend(cov);
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diff {
    /// Colour plane: mean and largest absolute cell difference.
    pub mean: f64,
    pub max: u8,
    /// Coverage plane: largest cell difference and relative change of total ink.
    pub cov_max: u8,
    pub ink: f64,
}

impl Diff {
    pub fn within_tolerance(&self) -> bool {
        self.mean <= TOL_MEAN
            && self.max <= TOL_MAX
            && self.cov_max <= TOL_COV_MAX
            && self.ink <= TOL_INK
    }
}

pub fn compare(a: &[u8], b: &[u8]) -> Option<Diff> {
    if a.len() != b.len() || a.len() != GRID * GRID * 4 {
        return None;
    }
    let split = GRID * GRID * 3;
    let (mut sum, mut max) = (0u64, 0u8);
    for (x, y) in a[..split].iter().zip(&b[..split]) {
        let d = x.abs_diff(*y);
        sum += u64::from(d);
        max = max.max(d);
    }
    let cov_max = a[split..]
        .iter()
        .zip(&b[split..])
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    let ink = |s: &[u8]| s[split..].iter().map(|&v| u64::from(v)).sum::<u64>() as f64;
    let (ia, ib) = (ink(a), ink(b));
    Some(Diff {
        mean: sum as f64 / split as f64,
        max,
        cov_max,
        ink: (ia - ib).abs() / ia.max(ib).max(1.0),
    })
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Tile {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                rgba.extend([c[0], c[1], c[2], 255]);
            }
        }
        Tile {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn hex_round_trip_and_hash() {
        let b = [0u8, 1, 254, 255];
        assert_eq!(from_hex(&to_hex(&b)).unwrap(), b);
        assert!(from_hex("abc").is_none());
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a64(b"a"), fnv1a64(b"b"));
    }

    #[test]
    fn signature_has_teeth_but_tolerates_antialiasing() {
        let base = tile(300, 400, |x, y| {
            if (60..180).contains(&x) && (80..260).contains(&y) {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        let s = signature(&base);
        assert_eq!(s.len(), GRID * GRID * 4);
        // One-pixel shift of the box: well inside tolerance.
        let shifted = tile(300, 400, |x, y| {
            if (61..181).contains(&x) && (80..260).contains(&y) {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        let d = compare(&s, &signature(&shifted)).unwrap();
        assert!(d.within_tolerance(), "{d:?}");
        // Missing content, a different colour and a large shift are all caught.
        let missing = tile(300, 400, |_, _| [255, 255, 255]);
        assert!(
            !compare(&s, &signature(&missing))
                .unwrap()
                .within_tolerance()
        );
        let red = tile(300, 400, |x, y| {
            if (60..180).contains(&x) && (80..260).contains(&y) {
                [200, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        assert!(!compare(&s, &signature(&red)).unwrap().within_tolerance());
        let moved = tile(300, 400, |x, y| {
            if (90..210).contains(&x) && (80..260).contains(&y) {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        assert!(!compare(&s, &signature(&moved)).unwrap().within_tolerance());
        assert!(compare(&s, &s[1..]).is_none());
        // A sparse page that loses its only mark (a missing form field) is caught by the ink total.
        let mark = |on: bool| {
            tile(300, 400, move |x, y| {
                if on && (100..140).contains(&x) && (100..110).contains(&y) {
                    [0, 0, 0]
                } else {
                    [255, 255, 255]
                }
            })
        };
        assert!(
            !compare(&signature(&mark(true)), &signature(&mark(false)))
                .unwrap()
                .within_tolerance()
        );
    }
}
