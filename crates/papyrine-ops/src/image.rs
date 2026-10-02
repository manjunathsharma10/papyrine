//! Object images: a serialized object plus raw stream bytes, and the machinery to restore one
//! in place.
//!
//! The serialized form is papyrine-cos's fingerprint representation (a shallow copy of the
//! object printed by qpdf, indirect references kept as `n g R`). `Document::parse_object` cannot
//! read references, so this module has its own small reader for that form and rebuilds the
//! object through the safe cos API. Reals keep their original text because leaves are handed
//! back to qpdf's own parser.

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::error::{Error, Result};

/// Pseudo object id for the trailer dictionary (not an indirect object).
pub const TRAILER: ObjId = ObjId {
    num: 0,
    generation: 0,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectImage {
    pub id: ObjId,
    /// Direct serialization (dictionary for streams).
    pub repr: Vec<u8>,
    /// Raw (still encoded) stream bytes, for stream objects.
    pub stream: Option<Vec<u8>>,
}

pub(crate) fn resolve(doc: &Document, id: ObjId) -> Result<Object> {
    Ok(if id == TRAILER {
        doc.trailer()?
    } else {
        doc.object(id)?
    })
}

impl ObjectImage {
    pub fn capture(doc: &Document, id: ObjId) -> Result<ObjectImage> {
        let obj = resolve(doc, id)?;
        let fp = doc.fingerprint(&obj)?;
        let stream = if fp.is_stream {
            Some(obj.stream_raw()?.to_vec())
        } else {
            None
        };
        Ok(ObjectImage {
            id,
            repr: fp.repr,
            stream,
        })
    }

    /// Approximate resident size in bytes.
    pub fn size(&self) -> usize {
        64 + self.repr.len() + self.stream.as_ref().map_or(0, Vec::len)
    }

    /// Does the live object equal this image exactly?
    pub fn matches(&self, doc: &Document) -> Result<bool> {
        Ok(ObjectImage::capture(doc, self.id)? == *self)
    }

    /// Make the live object equal to this image, in place where possible so existing handles
    /// stay valid.
    pub fn restore(&self, doc: &Document) -> Result<()> {
        let value = parse_value(&self.repr)?;
        let obj = resolve(doc, self.id)?;
        let kind = obj.kind()?;
        match (&self.stream, kind) {
            (Some(raw), ObjectKind::Stream) => {
                obj.stream_replace(raw, None, None)?;
                fill_dict(doc, &obj.stream_dict()?, &value)
            }
            (Some(_), _) => Err(Error::Corrupt(format!(
                "object {} is no longer a stream",
                self.id
            ))),
            (None, ObjectKind::Dictionary) if matches!(value, Value::Dict(_)) => {
                fill_dict(doc, &obj, &value)
            }
            (None, ObjectKind::Array) if matches!(value, Value::Array(_)) => {
                let Value::Array(items) = &value else {
                    unreachable!()
                };
                for i in (0..obj.array_len()?).rev() {
                    obj.array_remove(i)?;
                }
                for it in items {
                    obj.array_push(&build(doc, it)?)?;
                }
                Ok(())
            }
            (None, _) if self.id != TRAILER => {
                doc.replace_object(self.id, &build(doc, &value)?)?;
                Ok(())
            }
            (None, _) => Err(Error::Corrupt("trailer image is not a dictionary".into())),
        }
    }
}

/// Deep copy of an object's direct structure into a new direct object (references are kept as
/// references). Streams copy their dictionary only.
pub(crate) fn deep_copy_direct(doc: &Document, obj: &Object) -> Result<Object> {
    let fp = doc.fingerprint(obj)?;
    build(doc, &parse_value(&fp.repr)?)
}

fn fill_dict(doc: &Document, dict: &Object, value: &Value) -> Result<()> {
    let Value::Dict(kv) = value else {
        return Err(Error::Corrupt("expected a dictionary image".into()));
    };
    for k in dict.dict_keys()? {
        dict.dict_remove(&k)?;
    }
    for (k, v) in kv {
        dict.dict_set(k, &build(doc, v)?)?;
    }
    Ok(())
}

#[derive(Debug)]
enum Value {
    /// Scalar token text (number, name, string, bool, null), parsed by qpdf.
    Leaf(Vec<u8>),
    Ref(ObjId),
    Array(Vec<Value>),
    /// Keys are decoded names without the slash.
    Dict(Vec<(Vec<u8>, Value)>),
}

fn build(doc: &Document, v: &Value) -> Result<Object> {
    Ok(match v {
        Value::Leaf(t) => doc.parse_object(t)?,
        Value::Ref(id) => doc.object(*id)?,
        Value::Array(items) => {
            let a = doc.new_array();
            for i in items {
                a.array_push(&build(doc, i)?)?;
            }
            a
        }
        Value::Dict(kv) => {
            let d = doc.new_dict();
            for (k, v) in kv {
                d.dict_set(k, &build(doc, v)?)?;
            }
            d
        }
    })
}

const MAX_DEPTH: usize = 256;

fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}
fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

struct Reader<'a> {
    s: &'a [u8],
    i: usize,
}

fn parse_value(s: &[u8]) -> Result<Value> {
    let mut r = Reader { s, i: 0 };
    let v = r.value(0)?;
    r.skip_ws();
    if r.i != s.len() {
        return Err(Error::Corrupt("trailing bytes in object image".into()));
    }
    Ok(v)
}

impl Reader<'_> {
    fn err<T>(&self, what: &str) -> Result<T> {
        Err(Error::Corrupt(format!("{what} at byte {}", self.i)))
    }

    fn skip_ws(&mut self) {
        while self.i < self.s.len() && is_ws(self.s[self.i]) {
            self.i += 1;
        }
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.s.get(self.i + off).copied()
    }

    fn token(&mut self) -> &[u8] {
        let st = self.i;
        while self.i < self.s.len() && !is_ws(self.s[self.i]) && !is_delim(self.s[self.i]) {
            self.i += 1;
        }
        &self.s[st..self.i]
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return self.err("nesting too deep");
        }
        self.skip_ws();
        let Some(c) = self.peek(0) else {
            return self.err("unexpected end");
        };
        match c {
            b'<' if self.peek(1) == Some(b'<') => {
                self.i += 2;
                let mut kv = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek(0) {
                        Some(b'>') if self.peek(1) == Some(b'>') => {
                            self.i += 2;
                            return Ok(Value::Dict(kv));
                        }
                        Some(b'/') => {
                            self.i += 1;
                            let key = decode_name(self.token());
                            kv.push((key, self.value(depth + 1)?));
                        }
                        _ => return self.err("bad dictionary"),
                    }
                }
            }
            b'<' => {
                let st = self.i;
                while self.peek(0).is_some_and(|c| c != b'>') {
                    self.i += 1;
                }
                if self.peek(0).is_none() {
                    return self.err("unterminated hex string");
                }
                self.i += 1;
                Ok(Value::Leaf(self.s[st..self.i].to_vec()))
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek(0) {
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Array(items));
                        }
                        None => return self.err("unterminated array"),
                        _ => items.push(self.value(depth + 1)?),
                    }
                }
            }
            b'(' => {
                let st = self.i;
                let mut nest = 0usize;
                while let Some(c) = self.peek(0) {
                    self.i += 1;
                    match c {
                        b'\\' => self.i += 1,
                        b'(' => nest += 1,
                        b')' => {
                            nest -= 1;
                            if nest == 0 {
                                return Ok(Value::Leaf(self.s[st..self.i].to_vec()));
                            }
                        }
                        _ => {}
                    }
                }
                self.err("unterminated string")
            }
            b'/' => {
                let st = self.i;
                self.i += 1;
                self.token();
                Ok(Value::Leaf(self.s[st..self.i].to_vec()))
            }
            _ => {
                let tok = self.token().to_vec();
                if tok.is_empty() {
                    return self.err("unexpected delimiter");
                }
                if tok.iter().all(u8::is_ascii_digit)
                    && let Some(id) = self.try_ref(&tok)
                {
                    return Ok(Value::Ref(id));
                }
                Ok(Value::Leaf(tok))
            }
        }
    }

    /// After an integer token: is this `g R`?
    fn try_ref(&mut self, num: &[u8]) -> Option<ObjId> {
        let save = self.i;
        self.skip_ws();
        let g = self.token().to_vec();
        self.skip_ws();
        let r = self.token().to_vec();
        if !g.is_empty() && g.iter().all(u8::is_ascii_digit) && r == b"R" {
            let n = std::str::from_utf8(num).ok()?.parse().ok()?;
            let g = std::str::from_utf8(&g).ok()?.parse().ok()?;
            return Some(ObjId::new(n, g));
        }
        self.i = save;
        None
    }
}

fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#'
            && i + 2 < raw.len()
            && let Ok(h) = std::str::from_utf8(&raw[i + 1..i + 3])
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

// ---- compact binary codec used by the history spill ------------------------------------------

pub(crate) fn encode_images(groups: &[&[ObjectImage]]) -> Vec<u8> {
    let mut out = Vec::new();
    for g in groups {
        out.extend_from_slice(&(g.len() as u32).to_le_bytes());
        for img in *g {
            out.extend_from_slice(&img.id.num.to_le_bytes());
            out.extend_from_slice(&img.id.generation.to_le_bytes());
            out.extend_from_slice(&(img.repr.len() as u64).to_le_bytes());
            out.extend_from_slice(&img.repr);
            match &img.stream {
                None => out.push(0),
                Some(s) => {
                    out.push(1);
                    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
                    out.extend_from_slice(s);
                }
            }
        }
    }
    out
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.0.len() < n {
            return Err(Error::Corrupt("truncated spill record".into()));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<usize> {
        usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
            .map_err(|_| Error::Corrupt("length overflow".into()))
    }
}

pub(crate) fn decode_images(bytes: &[u8], groups: usize) -> Result<Vec<Vec<ObjectImage>>> {
    let mut c = Cursor(bytes);
    let mut out = Vec::with_capacity(groups);
    for _ in 0..groups {
        let n = c.u32()? as usize;
        let mut v = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let num = c.u32()?;
            let generation = u16::from_le_bytes(c.take(2)?.try_into().unwrap());
            let l = c.u64()?;
            let repr = c.take(l)?.to_vec();
            let stream = match c.take(1)?[0] {
                0 => None,
                _ => {
                    let l = c.u64()?;
                    Some(c.take(l)?.to_vec())
                }
            };
            v.push(ObjectImage {
                id: ObjId::new(num, generation),
                repr,
                stream,
            });
        }
        out.push(v);
    }
    Ok(out)
}
