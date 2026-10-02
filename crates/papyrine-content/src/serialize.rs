//! Serializer: minimal valid output, configurable number precision.

use crate::lexer::{is_delimiter, is_regular, is_whitespace};
use crate::object::{Object, Op};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SerializeOptions {
    /// Digits after the decimal point for reals (trailing zeros are trimmed).
    /// `None` writes the shortest text that reads back as the same `f64`.
    /// `parse(serialize(ops))` is only guaranteed identical when this is
    /// `None` or covers the precision of the input numbers.
    pub decimals: Option<u8>,
    /// Put each operation on its own line (costs one byte per op, which a
    /// separator is needed for anyway in most cases).
    pub newline_between_ops: bool,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self {
            decimals: Some(6),
            newline_between_ops: true,
        }
    }
}

impl SerializeOptions {
    pub fn lossless() -> Self {
        Self {
            decimals: None,
            ..Self::default()
        }
    }

    pub fn with_decimals(decimals: u8) -> Self {
        Self {
            decimals: Some(decimals),
            ..Self::default()
        }
    }
}

pub fn serialize(ops: &[Op], opts: &SerializeOptions) -> Vec<u8> {
    let mut out = Vec::with_capacity(ops.len() * 16);
    for op in ops {
        write_op(&mut out, op, opts);
    }
    out
}

/// Insert a space only if the previous byte and the next token would fuse.
/// A trailing `/` is an empty name, which a following regular byte would extend.
fn sep(out: &mut Vec<u8>) {
    if out.last().is_some_and(|&b| is_regular(b) || b == b'/') {
        out.push(b' ');
    }
}

pub fn write_op(out: &mut Vec<u8>, op: &Op, opts: &SerializeOptions) {
    if op.operator == b"BI" && op.inline_data.is_some() {
        write_inline_image(out, op, opts);
    } else {
        for o in &op.operands {
            write_object(out, o, opts);
        }
        sep(out);
        out.extend_from_slice(&op.operator);
    }
    out.push(if opts.newline_between_ops {
        b'\n'
    } else {
        b' '
    });
}

fn write_inline_image(out: &mut Vec<u8>, op: &Op, opts: &SerializeOptions) {
    out.extend_from_slice(b"BI");
    if let Some(Object::Dict(pairs)) = op.operands.first() {
        for (k, v) in pairs {
            write_name(out, k);
            write_object(out, v, opts);
        }
    }
    out.extend_from_slice(b"\nID ");
    let data = op.inline_data.as_deref().unwrap_or_default();
    out.extend_from_slice(data);
    // A stray "EI" near the end of the data would be judged by the bytes that
    // follow it, which include our terminator. A NUL separator is whitespace
    // but not text, so the reader rejects such a candidate, as it did when the
    // data was first parsed.
    let tail = &data[data.len().saturating_sub(8)..];
    let risky = tail.windows(2).any(|w| w == b"EI");
    out.push(if risky { 0 } else { b'\n' });
    out.extend_from_slice(b"EI");
}

pub fn write_object(out: &mut Vec<u8>, obj: &Object, opts: &SerializeOptions) {
    match obj {
        Object::Null => {
            sep(out);
            out.extend_from_slice(b"null");
        }
        Object::Bool(b) => {
            sep(out);
            out.extend_from_slice(if *b { b"true" } else { b"false" });
        }
        Object::Int(i) => {
            sep(out);
            write_int(out, *i);
        }
        Object::Real(r) => {
            sep(out);
            write_real(out, *r, opts.decimals);
        }
        Object::Str(s) => write_string(out, s),
        Object::Name(n) => write_name(out, n),
        Object::Array(a) => {
            out.push(b'[');
            for o in a {
                write_object(out, o, opts);
            }
            out.push(b']');
        }
        Object::Dict(d) => {
            out.extend_from_slice(b"<<");
            for (k, v) in d {
                write_name(out, k);
                write_object(out, v, opts);
            }
            out.extend_from_slice(b">>");
        }
    }
}

fn write_int(out: &mut Vec<u8>, i: i64) {
    let mut buf = [0u8; 20];
    let mut n = i.unsigned_abs();
    let mut pos = buf.len();
    loop {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if i < 0 {
        out.push(b'-');
    }
    out.extend_from_slice(&buf[pos..]);
}

pub(crate) fn write_real(out: &mut Vec<u8>, v: f64, decimals: Option<u8>) {
    if !v.is_finite() {
        out.push(b'0');
        return;
    }
    let s = match decimals {
        Some(d) => format!("{v:.*}", usize::from(d.min(20))),
        None => format!("{v}"),
    };
    let mut t = s.as_str();
    if t.contains('.') {
        t = t.trim_end_matches('0').trim_end_matches('.');
    }
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let digits = digits
        .strip_prefix('0')
        .filter(|r| r.starts_with('.'))
        .unwrap_or(digits);
    if digits.is_empty() || digits == "0" {
        out.push(b'0');
        return;
    }
    if neg {
        out.push(b'-');
    }
    out.extend_from_slice(digits.as_bytes());
}

fn write_name(out: &mut Vec<u8>, n: &[u8]) {
    out.push(b'/');
    for &b in n {
        if b <= b' ' || b >= 0x7F || b == b'#' || is_delimiter(b) || is_whitespace(b) {
            out.push(b'#');
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 15) as usize]);
        } else {
            out.push(b);
        }
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    let literal_len: usize = s
        .iter()
        .map(|&b| match b {
            b'(' | b')' | b'\\' | b'\n' | b'\r' => 2,
            0..=31 | 127..=255 => 4,
            _ => 1,
        })
        .sum::<usize>()
        + 2;
    if literal_len > s.len() * 2 + 2 {
        out.push(b'<');
        for &b in s {
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 15) as usize]);
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in s {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0..=31 | 127..=255 => {
                out.push(b'\\');
                out.push(b'0' + (b >> 6));
                out.push(b'0' + ((b >> 3) & 7));
                out.push(b'0' + (b & 7));
            }
            _ => out.push(b),
        }
    }
    out.push(b')');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn real(v: f64, d: Option<u8>) -> String {
        let mut o = Vec::new();
        write_real(&mut o, v, d);
        String::from_utf8(o).unwrap()
    }

    #[test]
    fn real_formatting_is_minimal() {
        assert_eq!(real(0.5, Some(6)), ".5");
        assert_eq!(real(-0.5, Some(6)), "-.5");
        assert_eq!(real(3.0, Some(6)), "3");
        assert_eq!(real(1.23456789, Some(3)), "1.235");
        assert_eq!(real(-0.0001, Some(2)), "0");
        assert_eq!(real(100.0, Some(2)), "100");
        assert_eq!(real(0.1 + 0.2, None), ".30000000000000004");
        assert_eq!(real(f64::NAN, None), "0");
        assert_eq!(real(1e-7, None), ".0000001");
    }

    #[test]
    fn minimal_spacing() {
        let p = parse(b"q 1 0 0 1 10 20 cm /F1 12 Tf [(a) 5 (b)] TJ Q");
        let out = serialize(
            &p.ops,
            &SerializeOptions {
                newline_between_ops: false,
                ..Default::default()
            },
        );
        assert_eq!(out, b"q 1 0 0 1 10 20 cm /F1 12 Tf [(a)5(b)]TJ Q ");
    }

    #[test]
    fn strings_and_names_escape() {
        let ops = vec![
            Op::new("Tj", vec![Object::string(b"a(b)\\\n\x01\xff")]),
            Op::new("Tj", vec![Object::string([0x80u8, 0x81, 0x82])]),
            Op::new("gs", vec![Object::Name(b"A B#/\x00".to_vec())]),
        ];
        let out = serialize(&ops, &SerializeOptions::lossless());
        let back = parse(&out);
        assert!(back.errors.is_empty());
        assert!(crate::ops_equal(&ops, &back.ops));
        assert!(String::from_utf8_lossy(&out).contains("<808182>"));
    }

    #[test]
    fn inline_image_roundtrip() {
        let src = b"BI /W 2 /H 2 /BPC 8 /CS /G ID \x01\x02\x03\x04\nEI\nq\n";
        let a = parse(src);
        let out = serialize(&a.ops, &SerializeOptions::lossless());
        let b = parse(&out);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        assert!(crate::ops_equal(&a.ops, &b.ops));
    }

    #[test]
    fn int_extremes() {
        let ops = vec![Op::new(
            "x",
            vec![Object::Int(i64::MIN), Object::Int(0), Object::Int(i64::MAX)],
        )];
        let out = serialize(&ops, &SerializeOptions::default());
        assert_eq!(out, b"-9223372036854775808 0 9223372036854775807 x\n");
    }
}
