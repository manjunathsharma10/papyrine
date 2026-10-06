//! Journal after-images: `papyrine_ops::ObjectImage` packed into the opaque bytes of
//! `papyrine_ipc::AfterImage::body`.
//!
//! Layout: `[flags u8][repr_len u32 LE][repr][has_stream u8][stream bytes]`. Bit 0 of `flags`
//! says the page tree may have changed (replay must rebuild qpdf's page cache around it).

use papyrine_cos::ObjId;
use papyrine_ipc::AfterImage;
use papyrine_ops::ObjectImage;

use crate::proto::{EngineError, Result};

const FLAG_PAGE_TREE: u8 = 1;

pub fn pack(img: &ObjectImage, page_tree: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + img.repr.len() + img.stream.as_ref().map_or(0, Vec::len));
    out.push(if page_tree { FLAG_PAGE_TREE } else { 0 });
    out.extend_from_slice(&(img.repr.len() as u32).to_le_bytes());
    out.extend_from_slice(&img.repr);
    match &img.stream {
        None => out.push(0),
        Some(s) => {
            out.push(1);
            out.extend_from_slice(s);
        }
    }
    out
}

fn corrupt(what: &str) -> EngineError {
    EngineError::Invalid(format!("corrupt after-image: {what}"))
}

pub fn unpack(id: ObjId, data: &[u8]) -> Result<(ObjectImage, bool)> {
    let flags = *data.first().ok_or_else(|| corrupt("empty"))?;
    let len = data
        .get(1..5)
        .ok_or_else(|| corrupt("short header"))?
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| corrupt("header"))? as usize;
    let repr = data.get(5..5 + len).ok_or_else(|| corrupt("short repr"))?;
    let tail = data.get(5 + len..).ok_or_else(|| corrupt("short tail"))?;
    let stream = match tail.split_first() {
        Some((0, [])) => None,
        Some((1, rest)) => Some(rest.to_vec()),
        _ => return Err(corrupt("stream marker")),
    };
    Ok((
        ObjectImage {
            id,
            repr: repr.to_vec(),
            stream,
        },
        flags & FLAG_PAGE_TREE != 0,
    ))
}

pub fn to_ipc(img: &ObjectImage, page_tree: bool) -> AfterImage {
    AfterImage {
        obj: img.id.num,
        generation: img.id.generation,
        body: Some(pack(img, page_tree)),
    }
}

/// Decode a whole commit's images; the page-tree flag is the OR over the images.
pub fn unpack_all(images: &[AfterImage]) -> Result<(Vec<ObjectImage>, bool)> {
    let mut out = Vec::with_capacity(images.len());
    let mut tree = false;
    for im in images {
        let Some(body) = &im.body else {
            return Err(EngineError::Invalid(format!(
                "after-image for object {} deletes it; PDF objects are never deleted",
                im.obj
            )));
        };
        let (img, t) = unpack(ObjId::new(im.obj, im.generation), body)?;
        tree |= t;
        out.push(img);
    }
    Ok((out, tree))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for stream in [None, Some(vec![1u8, 2, 3]), Some(vec![])] {
            let img = ObjectImage {
                id: ObjId::new(7, 0),
                repr: b"<< /A 1 >>".to_vec(),
                stream,
            };
            for t in [false, true] {
                let (back, tree) = unpack(img.id, &pack(&img, t)).unwrap();
                assert_eq!(back, img);
                assert_eq!(tree, t);
            }
        }
        assert!(unpack(ObjId::new(1, 0), &[]).is_err());
        assert!(unpack(ObjId::new(1, 0), &[0, 9, 0, 0, 0]).is_err());
    }
}
