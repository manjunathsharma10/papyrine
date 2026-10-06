//! Small total accessors over `papyrine_cos::Object`: absent, null and wrongly typed values all
//! read as `None`, so the field walker never fails on a hostile file.

use papyrine_cos::{DecodeLevel, Object, ObjectKind};
use papyrine_ops::decode_text_string;

pub(crate) const MAX_DEPTH: usize = 32;

pub(crate) fn kind(o: &Object) -> Option<ObjectKind> {
    o.kind().ok()
}

pub(crate) fn is_dict(o: &Object) -> bool {
    matches!(kind(o), Some(ObjectKind::Dictionary | ObjectKind::Stream))
}

/// `o[key]` when `o` is a dictionary (or stream) and the value is not null.
pub(crate) fn get(o: &Object, key: &str) -> Option<Object> {
    if !is_dict(o) {
        return None;
    }
    let v = o.dict_get(key).ok()?;
    if matches!(kind(&v), None | Some(ObjectKind::Null)) {
        None
    } else {
        Some(v)
    }
}

/// `key` looked up on `o` and then up the `/Parent` chain.
pub(crate) fn inherited(o: &Object, key: &str) -> Option<Object> {
    let mut cur = o.clone();
    for _ in 0..MAX_DEPTH {
        if let Some(v) = get(&cur, key) {
            return Some(v);
        }
        cur = get(&cur, "Parent")?;
    }
    None
}

pub(crate) fn int(o: &Object) -> Option<i64> {
    match kind(o)? {
        ObjectKind::Integer => o.as_int().ok(),
        ObjectKind::Real => o.as_f64().ok().map(|f| f as i64),
        _ => None,
    }
}

pub(crate) fn num(o: &Object) -> Option<f64> {
    match kind(o)? {
        ObjectKind::Integer | ObjectKind::Real => o.as_f64().ok(),
        _ => None,
    }
}

pub(crate) fn name(o: &Object) -> Option<String> {
    if kind(o)? == ObjectKind::Name {
        o.name()
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    } else {
        None
    }
}

pub(crate) fn boolean(o: &Object) -> Option<bool> {
    if kind(o)? == ObjectKind::Bool {
        o.as_bool().ok()
    } else {
        None
    }
}

/// A text string (or the decoded content of a text stream).
pub(crate) fn text(o: &Object) -> Option<String> {
    match kind(o)? {
        ObjectKind::String => o.string().ok().map(|b| decode_text_string(&b)),
        ObjectKind::Stream => o
            .stream_decoded(DecodeLevel::Generalized)
            .ok()
            .map(|b| decode_text_string(b.as_slice())),
        _ => None,
    }
}

pub(crate) fn items(o: &Object) -> Vec<Object> {
    if kind(o) == Some(ObjectKind::Array) {
        o.array_items().unwrap_or_default()
    } else {
        Vec::new()
    }
}

pub(crate) fn numbers(o: &Object) -> Vec<f64> {
    items(o).iter().filter_map(num).collect()
}

pub(crate) fn get_int(o: &Object, key: &str) -> Option<i64> {
    get(o, key).as_ref().and_then(int)
}

pub(crate) fn get_num(o: &Object, key: &str) -> Option<f64> {
    get(o, key).as_ref().and_then(num)
}

pub(crate) fn get_name(o: &Object, key: &str) -> Option<String> {
    get(o, key).as_ref().and_then(name)
}

pub(crate) fn get_text(o: &Object, key: &str) -> Option<String> {
    get(o, key).as_ref().and_then(text)
}

pub(crate) fn get_bool(o: &Object, key: &str) -> Option<bool> {
    get(o, key).as_ref().and_then(boolean)
}

/// A normalised `[x0 y0 x1 y1]` rectangle.
pub(crate) fn rect(o: &Object) -> Option<[f64; 4]> {
    let n = numbers(o);
    if n.len() < 4 {
        return None;
    }
    Some([
        n[0].min(n[2]),
        n[1].min(n[3]),
        n[0].max(n[2]),
        n[1].max(n[3]),
    ])
}
