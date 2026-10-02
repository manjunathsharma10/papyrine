//! Deterministic page content: text, raster images, vector drawings.

use crate::pdf::esc;
use crate::rng::Rng;

const SYLLABLES: &[&str] = &[
    "ka", "to", "mi", "re", "su", "lo", "ve", "an", "ir", "po", "da", "ne", "sha", "gri", "tal",
    "bo", "fen", "qua", "ly", "zor", "pen", "dra", "cu", "mex", "ion", "tri", "al", "or", "um",
    "ex",
];

pub struct Vocab(Vec<String>);

impl Vocab {
    pub fn new(seed: u64, size: usize) -> Self {
        let mut r = Rng::new(seed);
        let words = (0..size)
            .map(|_| {
                let n = 1 + r.below(4) as usize;
                (0..n)
                    .map(|_| SYLLABLES[r.below(SYLLABLES.len() as u64) as usize])
                    .collect::<String>()
            })
            .collect();
        Vocab(words)
    }

    pub fn word(&self, r: &mut Rng) -> &str {
        // Zipf-ish: squaring the uniform sample favours low indices.
        let u = r.unit();
        &self.0[((u * u) * self.0.len() as f64) as usize]
    }
}

/// A page of body text. `lines` lines of about `words` words each, font /F1 at `size` points.
pub fn text_page(
    v: &Vocab,
    r: &mut Rng,
    title: &str,
    lines: usize,
    words: usize,
    size: f64,
) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("BT /F2 14 Tf 54 790 Td ({}) Tj ET\n", esc(title)));
    s.push_str(&format!("BT /F1 {size} Tf {} TL 54 765 Td\n", size * 1.2));
    for _ in 0..lines {
        let mut line = String::new();
        for i in 0..words {
            if i > 0 {
                line.push(' ');
            }
            line.push_str(v.word(r));
        }
        s.push_str(&format!("({}) Tj T*\n", esc(&line)));
    }
    s.push_str("ET\n");
    s.into_bytes()
}

/// A page of numeric table rows (high entropy, compresses poorly).
pub fn number_rows(r: &mut Rng, rows: usize, cols: usize, size: f64, x0: f64, y0: f64) -> Vec<u8> {
    let mut s = format!("BT /F3 {size} Tf {} TL {x0} {y0} Td\n", size * 1.15);
    for _ in 0..rows {
        let mut line = String::new();
        for c in 0..cols {
            if c > 0 {
                line.push(' ');
            }
            line.push_str(&format!("{:>9.3}", r.range(-99999.0, 99999.0)));
        }
        s.push_str(&format!("({line}) Tj T*\n"));
    }
    s.push_str("ET\n");
    s.into_bytes()
}

/// RGB image: colour field quantised to `cell`-pixel blocks, soft ellipses, and `speckle` pixels per thousand of random noise.
pub fn image_rgb(w: usize, h: usize, seed: u64, speckle: u32, cell: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let blobs: Vec<(f64, f64, f64, [u8; 3])> = (0..6)
        .map(|_| {
            (
                r.range(0.0, w as f64),
                r.range(0.0, h as f64),
                r.range(0.05, 0.3) * w as f64,
                [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8],
            )
        })
        .collect();
    let (p, q) = (r.range(0.5, 2.0), r.range(0.5, 2.0));
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        let fy = (y / cell * cell) as f64 / h as f64;
        let row = &mut out[y * w * 3..(y + 1) * w * 3];
        for x in 0..w {
            let fx = (x / cell * cell) as f64 / w as f64;
            let mut px = [
                (fx * 255.0 * p) as u8,
                (fy * 255.0 * q) as u8,
                ((fx + fy) * 127.0) as u8,
            ];
            for &(cx, cy, rad, col) in &blobs {
                let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                if dx * dx + dy * dy < rad * rad {
                    px = col;
                }
            }
            row[x * 3..x * 3 + 3].copy_from_slice(&px);
        }
        let n = w * speckle as usize / 1000;
        for _ in 0..n {
            let x = r.below(w as u64) as usize;
            let mut b = [0u8; 3];
            r.fill(&mut b);
            row[x * 3..x * 3 + 3].copy_from_slice(&b);
        }
    }
    out
}

/// Large vector drawing across a `side` x `side` point page.
pub fn vector_drawing(side: f64, elements: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let mut s = String::with_capacity(elements * 60);
    s.push_str("q 0.5 w 0.85 g\n");
    let step = side / 50.0;
    for i in 0..=50 {
        let p = i as f64 * step;
        s.push_str(&format!(
            "0 {p:.1} m {side:.1} {p:.1} l {p:.1} 0 m {p:.1} {side:.1} l S\n"
        ));
    }
    for _ in 0..elements {
        let (x, y) = (r.range(0.0, side), r.range(0.0, side));
        let (cr, cg, cb) = (r.unit(), r.unit(), r.unit());
        match r.below(4) {
            0 => {
                let (w, h) = (r.range(5.0, 400.0), r.range(5.0, 400.0));
                s.push_str(&format!(
                    "{cr:.2} {cg:.2} {cb:.2} rg {x:.1} {y:.1} {w:.1} {h:.1} re f\n"
                ));
            }
            1 => {
                let lw = r.range(0.25, 12.0);
                let (dx, dy) = (r.range(-300.0, 300.0), r.range(-300.0, 300.0));
                s.push_str(&format!(
                    "{cr:.2} {cg:.2} {cb:.2} RG {lw:.2} w {x:.1} {y:.1} m {:.1} {:.1} l S\n",
                    x + dx,
                    y + dy
                ));
            }
            2 => {
                let d = |r: &mut Rng| r.range(-250.0, 250.0);
                let (a, b, c, e, f, g) = (
                    d(&mut r),
                    d(&mut r),
                    d(&mut r),
                    d(&mut r),
                    d(&mut r),
                    d(&mut r),
                );
                s.push_str(&format!(
                    "{cr:.2} {cg:.2} {cb:.2} rg {x:.1} {y:.1} m {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} c f\n",
                    x + a, y + b, x + c, y + e, x + f, y + g
                ));
            }
            _ => {
                let rad = r.range(3.0, 150.0);
                s.push_str(&format!(
                    "{cr:.2} {cg:.2} {cb:.2} RG {x:.1} {:.1} m {:.1} {y:.1} l {x:.1} {:.1} l {:.1} {y:.1} l h S\n",
                    y - rad, x + rad, y + rad, x - rad
                ));
            }
        }
    }
    s.push_str("Q\n");
    s.into_bytes()
}
