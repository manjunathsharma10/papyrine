//! COS object serializer: walks `papyrine_cos::Object` handles and emits PDF syntax.
//!
//! Rules that keep the output readable by every conforming parser:
//! * names escape everything outside `!`..`~` and the delimiters as `#xx`;
//! * strings are written as literal strings when printable, otherwise as hex; encrypted strings
//!   are always hex;
//! * reals never use exponent notation;
//! * nested indirect objects are written as `n g R`; the object being serialized is written as a
//!   body;
//! * stream dictionaries get a fresh direct `/Length`; filters and raw (still encoded) bytes are
//!   carried over unchanged.

use std::io::Write;

use papyrine_cos::{ObjId, Object, ObjectKind};

use crate::crypto::{Crypto, Method};
use crate::{Error, Result};

const MAX_DEPTH: usize = 400;

/// Encrypt strings with this object's key.
#[derive(Clone, Copy)]
pub(crate) struct StrEnc<'a> {
    pub crypto: &'a Crypto,
    pub id: ObjId,
}

pub fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        let plain = (0x21..=0x7e).contains(&b)
            && !matches!(
                b,
                b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#'
            );
        if plain {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

pub fn write_hex_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'<');
    for b in s {
        out.extend_from_slice(format!("{b:02X}").as_bytes());
    }
    out.push(b'>');
}

pub fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    let printable = s
        .iter()
        .filter(|&&b| (0x20..0x7f).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t')
        .count();
    // Hex is shorter than escaped literals for mostly binary data.
    if printable * 4 < s.len() * 3 {
        return write_hex_string(out, s);
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
            b'\t' => out.extend_from_slice(b"\\t"),
            0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
}

/// A plain decimal real. Uses the original text when it already is one; otherwise formats the
/// value with at most 6 fractional digits (never `1e-5`).
pub fn real_text(original: &str, value: f64) -> String {
    let t = original.trim();
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let plain = !body.is_empty()
        && body != "."
        && body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && body.bytes().filter(|&b| b == b'.').count() <= 1;
    if plain {
        return t.to_string();
    }
    format_real(value)
}

pub fn format_real(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    match s {
        "" | "-" | "-0" => "0".into(),
        s => s.to_string(),
    }
}

/// Write `obj` as a direct value. `top` is true for the value of an indirect object itself.
fn write_value(
    out: &mut Vec<u8>,
    obj: &Object,
    enc: Option<StrEnc<'_>>,
    top: bool,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::unsupported("object nesting deeper than 400 levels"));
    }
    if !top && obj.is_indirect() {
        let id = obj
            .id()
            .ok_or_else(|| Error::unsupported("indirect object without id"))?;
        out.extend_from_slice(format!("{} {} R", id.num, id.generation).as_bytes());
        return Ok(());
    }
    match obj.kind()? {
        ObjectKind::Null => out.extend_from_slice(b"null"),
        ObjectKind::Bool => out.extend_from_slice(if obj.as_bool()? { b"true" } else { b"false" }),
        ObjectKind::Integer => out.extend_from_slice(obj.as_int()?.to_string().as_bytes()),
        ObjectKind::Real => {
            let txt = real_text(&obj.real_text()?, obj.as_f64()?);
            out.extend_from_slice(txt.as_bytes());
        }
        ObjectKind::String => {
            let s = obj.string()?;
            match enc {
                Some(e) if e.crypto.string_method() != Method::Identity => {
                    let ct = e.crypto.encrypt(e.crypto.string_method(), e.id, &s)?;
                    write_hex_string(out, &ct);
                }
                _ => write_string(out, &s),
            }
        }
        ObjectKind::Name => write_name(out, &obj.name()?),
        ObjectKind::Array => {
            out.push(b'[');
            for (i, item) in obj.array_items()?.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_value(out, item, enc, false, depth + 1)?;
            }
            out.push(b']');
        }
        ObjectKind::Dictionary => write_dict(out, obj, enc, depth, &[])?,
        k => {
            return Err(Error::unsupported(format!(
                "cannot serialize object of kind {k:?}"
            )));
        }
    }
    Ok(())
}

fn write_dict(
    out: &mut Vec<u8>,
    dict: &Object,
    enc: Option<StrEnc<'_>>,
    depth: usize,
    skip: &[&[u8]],
) -> Result<()> {
    out.extend_from_slice(b"<<");
    for key in dict.dict_keys()? {
        if skip.contains(&key.as_slice()) {
            continue;
        }
        out.push(b' ');
        write_name(out, &key);
        out.push(b' ');
        let v = dict.dict_get(&key)?;
        write_value(out, &v, enc, false, depth + 1)?;
    }
    out.extend_from_slice(b" >>");
    Ok(())
}

/// Serialize `obj` as the value of a direct slot (references stay `n g R`). Used for trailer
/// entries. Strings are not encrypted.
pub fn write_direct(out: &mut Vec<u8>, obj: &Object) -> Result<()> {
    write_value(out, obj, None, false, 0)
}

/// Is the stream excluded from encryption (`/Type /XRef`, cleartext metadata, Identity crypt
/// filter)? Also rejects shapes we cannot encrypt faithfully.
fn stream_encryption(crypto: &Crypto, dict: &Object) -> Result<Method> {
    let ty = dict.dict_get("Type")?;
    let ty = if ty.kind()? == ObjectKind::Name {
        ty.name()?
    } else {
        Vec::new()
    };
    if ty == b"XRef" {
        return Ok(Method::Identity);
    }
    if ty == b"Metadata" && !crypto.encrypt_metadata() {
        return Ok(Method::Identity);
    }
    if ty == b"EmbeddedFile" && crypto.has_eff() {
        return Err(Error::unsupported(
            "embedded-file stream in a document with a separate /EFF crypt filter",
        ));
    }
    // A /Crypt filter selects a named crypt filter; /Identity (the default) leaves it clear.
    let filter = dict.dict_get("Filter")?;
    let names: Vec<Vec<u8>> = match filter.kind()? {
        ObjectKind::Name => vec![filter.name()?],
        ObjectKind::Array => {
            let mut v = Vec::new();
            for f in filter.array_items()? {
                v.push(if f.kind()? == ObjectKind::Name {
                    f.name()?
                } else {
                    Vec::new()
                });
            }
            v
        }
        _ => Vec::new(),
    };
    if let Some(idx) = names.iter().position(|n| n == b"Crypt") {
        let parms = dict.dict_get("DecodeParms")?;
        let p = match parms.kind()? {
            ObjectKind::Dictionary if idx == 0 => Some(parms),
            ObjectKind::Array => Some(parms.array_get(idx)?),
            _ => None,
        };
        let name = match p {
            Some(p) if p.kind()? == ObjectKind::Dictionary && p.dict_has("Name")? => {
                p.dict_get("Name")?.name()?
            }
            _ => b"Identity".to_vec(),
        };
        if name == b"Identity" {
            return Ok(Method::Identity);
        }
        return Err(Error::unsupported(
            "stream with a named (non-Identity) crypt filter",
        ));
    }
    Ok(crypto.stream_method())
}

/// Write `n g obj ... endobj` for an indirect object. Returns nothing; the caller tracks offsets.
pub fn write_indirect<W: Write>(
    w: &mut W,
    id: ObjId,
    obj: &Object,
    crypto: Option<&Crypto>,
) -> Result<()> {
    // The /Encrypt dictionary itself is stored in the clear.
    let crypto = crypto.filter(|c| !c.is_encrypt_dict(id));
    let enc = crypto.map(|c| StrEnc { crypto: c, id });
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(format!("{} {} obj\n", id.num, id.generation).as_bytes());
    if obj.kind()? == ObjectKind::Stream {
        let dict = obj.stream_dict()?;
        let raw = obj.stream_raw()?;
        let method = match crypto {
            Some(c) => stream_encryption(c, &dict)?,
            None => Method::Identity,
        };
        let encrypted;
        let data: &[u8] = if method == Method::Identity {
            raw.as_slice()
        } else {
            encrypted =
                crypto
                    .expect("method implies crypto")
                    .encrypt(method, id, raw.as_slice())?;
            &encrypted
        };
        buf.extend_from_slice(b"<<");
        for key in dict.dict_keys()? {
            if key == b"Length" {
                continue;
            }
            buf.push(b' ');
            write_name(&mut buf, &key);
            buf.push(b' ');
            write_value(&mut buf, &dict.dict_get(&key)?, enc, false, 1)?;
        }
        buf.extend_from_slice(format!(" /Length {} >>\nstream\n", data.len()).as_bytes());
        w.write_all(&buf)?;
        w.write_all(data)?;
        w.write_all(b"\nendstream\nendobj\n")?;
    } else {
        write_value(&mut buf, obj, enc, true, 0)?;
        buf.extend_from_slice(b"\nendobj\n");
        w.write_all(&buf)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_escape() {
        let mut o = Vec::new();
        write_name(&mut o, b"A B/C#\xE9");
        assert_eq!(o, b"/A#20B#2FC#23#E9");
    }

    #[test]
    fn strings_literal_and_hex() {
        let mut o = Vec::new();
        write_string(&mut o, b"a(b)c\\\r\n");
        assert_eq!(o, b"(a\\(b\\)c\\\\\\r\\n)");
        let mut o = Vec::new();
        write_string(&mut o, &[0, 1, 2, 0xff]);
        assert_eq!(o, b"<000102FF>");
        let mut o = Vec::new();
        write_string(&mut o, b"");
        assert_eq!(o, b"()");
    }

    #[test]
    fn reals_without_exponent() {
        assert_eq!(real_text("1e-5", 1e-5), "0.00001");
        assert_eq!(real_text("0.50", 0.5), "0.50");
        assert_eq!(real_text("-.5", -0.5), "-.5");
        assert_eq!(real_text("1.", 1.0), "1.");
        assert_eq!(format_real(1e-9), "0");
        assert_eq!(format_real(-1e-9), "0");
        assert_eq!(format_real(12.3456789), "12.345679");
        assert_eq!(format_real(f64::NAN), "0");
        assert_eq!(real_text("+3", 3.0), "+3");
    }
}
