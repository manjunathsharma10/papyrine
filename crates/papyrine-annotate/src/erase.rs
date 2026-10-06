//! The ink eraser: remove the parts of pen strokes within a radius of an eraser path.

use crate::geometry::Pt;

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Parameter interval `[lo, hi]` within `0..=1` of the segment `a..b` that lies inside the
/// capsule (all points within `r` of the segment `e..f`), or `None`. The capsule is convex, so
/// the intersection is one interval: the hull of the two end discs and the strip between them.
fn capsule_interval(a: Pt, b: Pt, e: Pt, f: Pt, r: f64) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut add = |x: Option<(f64, f64)>| {
        if let Some((l, h)) = x {
            lo = lo.min(l);
            hi = hi.max(h);
        }
    };
    add(disk_interval(a, b, e, r));
    add(disk_interval(a, b, f, r));
    let d = sub(f, e);
    let l2 = dot(d, d);
    if l2 > 1e-12 {
        let n = [-d[1], d[0]];
        let nl = l2.sqrt();
        let ab = sub(b, a);
        let ae = sub(a, e);
        // Signed distance from the line e-f and projection fraction along it, both linear in t.
        let (dist0, dist1) = (dot(ae, n) / nl, dot(ab, n) / nl);
        let (u0, u1) = (dot(ae, d) / l2, dot(ab, d) / l2);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        if clip(&mut t0, &mut t1, dist0, dist1, -r, r) && clip(&mut t0, &mut t1, u0, u1, 0.0, 1.0) {
            add(Some((t0, t1)));
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Restrict `[t0, t1]` so that `lo <= c0 + t*c1 <= hi`. False when nothing is left.
fn clip(t0: &mut f64, t1: &mut f64, c0: f64, c1: f64, lo: f64, hi: f64) -> bool {
    if c1.abs() < 1e-12 {
        return c0 >= lo - 1e-9 && c0 <= hi + 1e-9;
    }
    let (mut a, mut b) = ((lo - c0) / c1, (hi - c0) / c1);
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    *t0 = t0.max(a);
    *t1 = t1.min(b);
    *t0 <= *t1
}

fn disk_interval(a: Pt, b: Pt, c: Pt, r: f64) -> Option<(f64, f64)> {
    let d = sub(b, a);
    let f = sub(a, c);
    let qa = dot(d, d);
    if qa < 1e-18 {
        return (dot(f, f) <= r * r).then_some((0.0, 1.0));
    }
    let qb = 2.0 * dot(f, d);
    let qc = dot(f, f) - r * r;
    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let (t0, t1) = ((-qb - s) / (2.0 * qa), (-qb + s) / (2.0 * qa));
    let (lo, hi) = (t0.max(0.0), t1.min(1.0));
    (lo <= hi).then_some((lo, hi))
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Kept parameter intervals of segment `a..b` after removing what the eraser covers.
fn kept_intervals(a: Pt, b: Pt, eraser: &[(Pt, Pt)], radius: f64) -> Vec<(f64, f64)> {
    let mut iv: Vec<(f64, f64)> = eraser
        .iter()
        .filter_map(|(e, f)| capsule_interval(a, b, *e, *f, radius))
        .collect();
    iv.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut kept = Vec::new();
    let mut t = 0.0f64;
    for (l, h) in iv {
        if l > t + 1e-9 {
            kept.push((t, l));
        }
        t = t.max(h);
    }
    if t < 1.0 - 1e-9 {
        kept.push((t, 1.0));
    }
    kept
}

/// Remove everything within `radius` of the polyline `path` (a single point erases a disc).
/// Strokes are split where erased; pieces that shrink to a single point are dropped.
pub fn erase_strokes(strokes: &[Vec<Pt>], path: &[Pt], radius: f64) -> Vec<Vec<Pt>> {
    if path.is_empty() || radius <= 0.0 {
        return strokes.to_vec();
    }
    let eraser: Vec<(Pt, Pt)> = if path.len() == 1 {
        vec![(path[0], path[0])]
    } else {
        path.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let mut out: Vec<Vec<Pt>> = Vec::new();
    for s in strokes {
        if s.len() == 1 {
            let p = s[0];
            let hit = eraser
                .iter()
                .any(|(e, f)| capsule_interval(p, p, *e, *f, radius).is_some());
            if !hit {
                out.push(s.clone());
            }
            continue;
        }
        let mut cur: Vec<Pt> = Vec::new();
        for w in s.windows(2) {
            let (a, b) = (w[0], w[1]);
            let kept = kept_intervals(a, b, &eraser, radius);
            if kept.is_empty() {
                flush(&mut cur, &mut out);
            }
            for (l, h) in kept {
                let continues = l <= 1e-9 && !cur.is_empty();
                if !continues {
                    flush(&mut cur, &mut out);
                    cur.push(lerp(a, b, l));
                }
                cur.push(lerp(a, b, h));
                if h < 1.0 - 1e-9 {
                    flush(&mut cur, &mut out);
                }
            }
        }
        flush(&mut cur, &mut out);
    }
    out
}

fn flush(cur: &mut Vec<Pt>, out: &mut Vec<Vec<Pt>>) {
    // A piece needs two distinct points.
    cur.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if cur.len() >= 2 {
        out.push(std::mem::take(cur));
    } else {
        cur.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_len(s: &[Vec<Pt>]) -> f64 {
        s.iter()
            .flat_map(|st| {
                st.windows(2)
                    .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            })
            .sum()
    }

    #[test]
    fn erasing_the_middle_splits_a_stroke() {
        let s = vec![vec![[0.0, 0.0], [100.0, 0.0]]];
        let r = erase_strokes(&s, &[[50.0, 0.0]], 10.0);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!((r[0][1][0] - 40.0).abs() < 1e-6);
        assert!((r[1][0][0] - 60.0).abs() < 1e-6);
        assert!((total_len(&r) - 80.0).abs() < 1e-6);
    }

    #[test]
    fn eraser_stroke_cuts_across() {
        let s = vec![vec![[0.0, 0.0], [100.0, 0.0]]];
        let r = erase_strokes(&s, &[[30.0, -20.0], [30.0, 20.0]], 2.0);
        assert_eq!(r.len(), 2);
        assert!((r[0][1][0] - 28.0).abs() < 1e-6);
        assert!((r[1][0][0] - 32.0).abs() < 1e-6);
    }

    #[test]
    fn untouched_and_fully_erased() {
        let s = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]];
        assert_eq!(erase_strokes(&s, &[[100.0, 100.0]], 3.0), s);
        assert!(erase_strokes(&s, &[[0.0, 0.0], [10.0, 10.0]], 20.0).is_empty());
    }

    #[test]
    fn polyline_corner_is_kept_when_erased_elsewhere() {
        let s = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]];
        let r = erase_strokes(&s, &[[0.0, 0.0]], 2.0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].len(), 4);
        assert!((r[0][0][0] - 2.0).abs() < 1e-6);
        assert_eq!(r[0][1], [10.0, 0.0]);
    }

    #[test]
    fn erasing_a_corner_leaves_two_pieces() {
        let s = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]];
        let r = erase_strokes(&s, &[[10.0, 0.0]], 1.0);
        assert_eq!(r.len(), 2);
        assert!((r[0][1][0] - 9.0).abs() < 1e-6);
        assert!((r[1][0][1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dots_are_erased_by_hit() {
        let s = vec![vec![[5.0, 5.0]], vec![[50.0, 50.0]]];
        let r = erase_strokes(&s, &[[5.0, 6.0]], 2.0);
        assert_eq!(r, vec![vec![[50.0, 50.0]]]);
    }
}
