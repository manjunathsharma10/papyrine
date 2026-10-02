//! Builder for appearance streams and other generated content.

use crate::object::{Object, Op};
use crate::serialize::{SerializeOptions, serialize};
use papyrine_core::Matrix;

#[derive(Clone, Debug, PartialEq)]
pub enum TextItem {
    Text(Vec<u8>),
    /// Kerning adjustment in thousandths of text space units.
    Adjust(f64),
}

#[derive(Clone, Debug)]
pub struct ContentBuilder {
    ops: Vec<Op>,
    opts: SerializeOptions,
}

impl Default for ContentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn n(v: f64) -> Object {
    Object::from(v)
}

impl ContentBuilder {
    /// Four decimals is plenty for page-space coordinates (1e-4 pt).
    pub fn new() -> Self {
        Self::with_options(SerializeOptions::with_decimals(4))
    }

    pub fn with_options(opts: SerializeOptions) -> Self {
        Self {
            ops: Vec::new(),
            opts,
        }
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn into_ops(self) -> Vec<Op> {
        self.ops
    }

    pub fn finish(&self) -> Vec<u8> {
        serialize(&self.ops, &self.opts)
    }

    /// True if `q`/`Q` and `BT`/`ET` are balanced and never go negative.
    pub fn is_balanced(&self) -> bool {
        let (mut q, mut bt) = (0i32, 0i32);
        for op in &self.ops {
            match op.operator.as_slice() {
                b"q" => q += 1,
                b"Q" => q -= 1,
                b"BT" => bt += 1,
                b"ET" => bt -= 1,
                _ => {}
            }
            if q < 0 || bt < 0 {
                return false;
            }
        }
        q == 0 && bt == 0
    }

    /// Append any operator.
    pub fn op(&mut self, operator: &str, operands: Vec<Object>) -> &mut Self {
        self.ops.push(Op::new(operator, operands));
        self
    }

    fn nums(&mut self, operator: &str, v: &[f64]) -> &mut Self {
        self.op(operator, v.iter().copied().map(n).collect())
    }

    // Graphics state
    pub fn save(&mut self) -> &mut Self {
        self.op("q", vec![])
    }
    pub fn restore(&mut self) -> &mut Self {
        self.op("Q", vec![])
    }
    pub fn concat_matrix(&mut self, m: &Matrix) -> &mut Self {
        self.nums("cm", &m.to_array())
    }
    pub fn line_width(&mut self, w: f64) -> &mut Self {
        self.nums("w", &[w])
    }
    pub fn line_cap(&mut self, c: i64) -> &mut Self {
        self.op("J", vec![Object::Int(c)])
    }
    pub fn line_join(&mut self, j: i64) -> &mut Self {
        self.op("j", vec![Object::Int(j)])
    }
    pub fn miter_limit(&mut self, m: f64) -> &mut Self {
        self.nums("M", &[m])
    }
    pub fn dash(&mut self, pattern: &[f64], phase: f64) -> &mut Self {
        let arr = Object::Array(pattern.iter().copied().map(n).collect());
        self.op("d", vec![arr, n(phase)])
    }
    /// `gs` with an ExtGState resource name.
    pub fn ext_gstate(&mut self, name: &str) -> &mut Self {
        self.op("gs", vec![Object::name(name)])
    }

    // Paths
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.nums("m", &[x, y])
    }
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.nums("l", &[x, y])
    }
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &mut Self {
        self.nums("c", &[x1, y1, x2, y2, x3, y3])
    }
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.nums("re", &[x, y, w, h])
    }
    pub fn close(&mut self) -> &mut Self {
        self.op("h", vec![])
    }
    pub fn fill(&mut self) -> &mut Self {
        self.op("f", vec![])
    }
    pub fn fill_even_odd(&mut self) -> &mut Self {
        self.op("f*", vec![])
    }
    pub fn stroke(&mut self) -> &mut Self {
        self.op("S", vec![])
    }
    pub fn close_stroke(&mut self) -> &mut Self {
        self.op("s", vec![])
    }
    pub fn fill_stroke(&mut self) -> &mut Self {
        self.op("B", vec![])
    }
    pub fn clip(&mut self) -> &mut Self {
        self.op("W", vec![])
    }
    pub fn clip_even_odd(&mut self) -> &mut Self {
        self.op("W*", vec![])
    }
    /// End path without painting (`n`), typically after a clip.
    pub fn end_path(&mut self) -> &mut Self {
        self.op("n", vec![])
    }

    // Colour
    pub fn fill_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.nums("rg", &[r, g, b])
    }
    pub fn stroke_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.nums("RG", &[r, g, b])
    }
    pub fn fill_gray(&mut self, g: f64) -> &mut Self {
        self.nums("g", &[g])
    }
    pub fn stroke_gray(&mut self, g: f64) -> &mut Self {
        self.nums("G", &[g])
    }
    pub fn fill_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.nums("k", &[c, m, y, k])
    }
    pub fn stroke_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.nums("K", &[c, m, y, k])
    }

    // Text
    pub fn begin_text(&mut self) -> &mut Self {
        self.op("BT", vec![])
    }
    pub fn end_text(&mut self) -> &mut Self {
        self.op("ET", vec![])
    }
    pub fn font(&mut self, name: &str, size: f64) -> &mut Self {
        self.op("Tf", vec![Object::name(name), n(size)])
    }
    pub fn text_move(&mut self, tx: f64, ty: f64) -> &mut Self {
        self.nums("Td", &[tx, ty])
    }
    pub fn text_matrix(&mut self, m: &Matrix) -> &mut Self {
        self.nums("Tm", &m.to_array())
    }
    pub fn leading(&mut self, l: f64) -> &mut Self {
        self.nums("TL", &[l])
    }
    pub fn next_line(&mut self) -> &mut Self {
        self.op("T*", vec![])
    }
    pub fn show_text(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        self.op("Tj", vec![Object::string(bytes)])
    }
    pub fn show_text_adjusted(&mut self, items: &[TextItem]) -> &mut Self {
        let arr = items
            .iter()
            .map(|i| match i {
                TextItem::Text(t) => Object::Str(t.clone()),
                TextItem::Adjust(a) => n(*a),
            })
            .collect();
        self.op("TJ", vec![Object::Array(arr)])
    }

    // XObjects
    pub fn do_xobject(&mut self, name: &str) -> &mut Self {
        self.op("Do", vec![Object::name(name)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    #[test]
    fn builds_a_highlight_appearance() {
        let mut b = ContentBuilder::new();
        b.save()
            .ext_gstate("GS0")
            .fill_rgb(1.0, 1.0, 0.0)
            .rect(10.0, 20.5, 100.25, 12.0)
            .fill()
            .restore();
        assert!(b.is_balanced());
        assert_eq!(
            b.finish(),
            b"q\n/GS0 gs\n1 1 0 rg\n10 20.5 100.25 12 re\nf\nQ\n"
        );
    }

    #[test]
    fn text_block_roundtrips_through_parser() {
        let mut b = ContentBuilder::with_options(SerializeOptions::lossless());
        b.begin_text()
            .font("Helv", 9.5)
            .text_matrix(&Matrix::translate(72.0, 700.0))
            .show_text("Hello (world)")
            .show_text_adjusted(&[
                TextItem::Text(b"A".to_vec()),
                TextItem::Adjust(-35.5),
                TextItem::Text(b"V".to_vec()),
            ])
            .end_text()
            .save()
            .concat_matrix(&Matrix::scale(2.0, 0.5))
            .dash(&[3.0, 2.0], 0.0)
            .do_xobject("Im0")
            .restore();
        assert!(b.is_balanced());
        let p = parse(&b.finish());
        assert!(p.errors.is_empty());
        assert!(crate::ops_equal(b.ops(), &p.ops));
    }

    #[test]
    fn unbalanced_detected() {
        let mut b = ContentBuilder::new();
        b.save().begin_text();
        assert!(!b.is_balanced());
        let mut b = ContentBuilder::new();
        b.restore();
        assert!(!b.is_balanced());
    }
}
