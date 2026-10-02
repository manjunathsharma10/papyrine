//! Incremental sections and the ID-preserving full write.
//!
//! Both serialize objects straight from the in-memory qpdf instance (so object numbers are the
//! ones the engine already uses) and finish with one cross-reference section: a classic table
//! when the file used tables, an xref stream otherwise.

use std::collections::BTreeMap;
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::chain::{ChainState, XrefKind};
use crate::crypto::Crypto;
use crate::syntax::{write_direct, write_hex_string, write_indirect, write_name};
use crate::{Error, Result};

/// Trailer keys the writer computes itself; everything else is carried over.
const COMPUTED_KEYS: &[&[u8]] = &[
    b"Size",
    b"Prev",
    b"XRefStm",
    b"Type",
    b"W",
    b"Index",
    b"Length",
    b"Filter",
    b"DecodeParms",
    b"DL",
    b"ID",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    InUse {
        offset: u64,
    },
    /// `next` is the next free object in the free list (0 ends it).
    Free {
        next: u32,
        generation: u16,
    },
}

/// Counts bytes written so object offsets are exact whatever the sink is.
struct Counter<W> {
    inner: W,
    pos: u64,
}

impl<W: Write> Write for Counter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// What to put in an incremental section.
#[derive(Debug, Clone, Default)]
pub struct SectionRequest {
    /// Changed and new objects (current content is read from the document).
    pub dirty: Vec<ObjId>,
    /// Objects that were deleted: written as free entries.
    pub freed: Vec<ObjId>,
    /// Second half of the trailer `/ID`; random when `None`. Tests pin it.
    pub new_id: Option<[u8; 16]>,
}

/// One appended section, ready to be concatenated after the bytes it was built against.
#[derive(Debug, Clone)]
pub struct Section {
    pub bytes: Vec<u8>,
    /// Length of the bytes this section goes after.
    pub base_len: u64,
    /// Absolute offset of every object written.
    pub objects: Vec<(ObjId, u64)>,
    /// State of the chain once this section is appended.
    pub chain: ChainState,
    pub new_id: [u8; 16],
}

fn random_id() -> Result<[u8; 16]> {
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    Ok(id)
}

/// Second half of the written `/ID`.
enum IdSecond {
    New([u8; 16]),
    /// Keep the document's own pair untouched.
    Keep,
}

/// Trailer entries as `(key, serialized value)`, with `/ID` replaced by `[orig new]`.
fn trailer_entries(doc: &Document, second: IdSecond) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let trailer = doc.trailer()?;
    let mut out = Vec::new();
    for key in trailer.dict_keys()? {
        if COMPUTED_KEYS.contains(&key.as_slice()) {
            continue;
        }
        let mut v = Vec::new();
        write_direct(&mut v, &trailer.dict_get(&key)?)?;
        out.push((key, v));
    }
    let id_obj = trailer.dict_get("ID")?;
    let id_string = |i: usize| -> Result<Option<Vec<u8>>> {
        if id_obj.kind()? == ObjectKind::Array && id_obj.array_len()? > i {
            let s = id_obj.array_get(i)?;
            if s.kind()? == ObjectKind::String {
                return Ok(Some(s.string()?));
            }
        }
        Ok(None)
    };
    let fresh = random_id()?;
    let first = id_string(0)?.unwrap_or_else(|| match &second {
        IdSecond::New(n) => n.to_vec(),
        IdSecond::Keep => fresh.to_vec(),
    });
    let second_bytes = match second {
        IdSecond::New(n) => n.to_vec(),
        IdSecond::Keep => id_string(1)?.unwrap_or_else(|| first.clone()),
    };
    let mut v = vec![b'['];
    write_hex_string(&mut v, &first);
    v.push(b' ');
    write_hex_string(&mut v, &second_bytes);
    v.push(b']');
    out.push((b"ID".to_vec(), v));
    out.sort();
    Ok(out)
}

fn trailer_dict_body(entries: &[(Vec<u8>, Vec<u8>)], skip: &[&[u8]]) -> Vec<u8> {
    let mut b = Vec::new();
    for (k, v) in entries {
        if skip.contains(&k.as_slice()) {
            continue;
        }
        b.push(b' ');
        write_name(&mut b, k);
        b.push(b' ');
        b.extend_from_slice(v);
    }
    b
}

/// Turn `entries` into xref subsections `(first, [entries])`; also chains the free entries.
fn subsections(entries: &BTreeMap<u32, Entry>) -> Vec<(u32, Vec<Entry>)> {
    let mut out: Vec<(u32, Vec<Entry>)> = Vec::new();
    for (&num, &e) in entries {
        match out.last_mut() {
            Some((first, list)) if *first + list.len() as u32 == num => list.push(e),
            _ => out.push((num, vec![e])),
        }
    }
    out
}

/// Link free entries into one list starting at object 0 and add object 0 itself.
fn chain_free(entries: &mut BTreeMap<u32, Entry>) {
    let free: Vec<u32> = entries
        .iter()
        .filter(|(_, e)| matches!(e, Entry::Free { .. }))
        .map(|(&n, _)| n)
        .collect();
    if free.is_empty() {
        return;
    }
    for (i, &n) in free.iter().enumerate() {
        let next = free.get(i + 1).copied().unwrap_or(0);
        if let Some(Entry::Free { generation, .. }) = entries.get(&n).copied() {
            entries.insert(n, Entry::Free { next, generation });
        }
    }
    entries.insert(
        0,
        Entry::Free {
            next: free[0],
            generation: 65535,
        },
    );
}

fn table_bytes(entries: &BTreeMap<u32, Entry>) -> Vec<u8> {
    let mut o = b"xref\n".to_vec();
    for (first, list) in subsections(entries) {
        o.extend_from_slice(format!("{first} {}\n", list.len()).as_bytes());
        for e in list {
            match e {
                Entry::InUse { offset } => {
                    o.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes())
                }
                Entry::Free { next, generation } => {
                    o.extend_from_slice(format!("{next:010} {generation:05} f \n").as_bytes())
                }
            }
        }
    }
    o
}

fn be_bytes(v: u64, width: usize) -> impl Iterator<Item = u8> {
    (0..width).rev().map(move |i| (v >> (8 * i)) as u8)
}

/// `(/W widths, /Index array text, deflated data)` for an xref stream.
fn xref_stream_data(entries: &BTreeMap<u32, Entry>) -> Result<([usize; 3], String, Vec<u8>)> {
    let max_off = entries
        .values()
        .filter_map(|e| match e {
            Entry::InUse { offset } => Some(*offset),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let max_next = entries
        .values()
        .filter_map(|e| match e {
            Entry::Free { next, .. } => Some(u64::from(*next)),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let w2 = (1..=8)
        .find(|&w| max_off.max(max_next) >> (8 * w) == 0)
        .unwrap_or(8);
    let widths = [1usize, w2, 2];
    let mut raw = Vec::new();
    let mut index = String::new();
    for (first, list) in subsections(entries) {
        index.push_str(&format!("{first} {} ", list.len()));
        for e in list {
            match e {
                Entry::InUse { offset } => {
                    raw.push(1);
                    raw.extend(be_bytes(offset, w2));
                    raw.extend(be_bytes(0, 2));
                }
                Entry::Free { next, generation } => {
                    raw.push(0);
                    raw.extend(be_bytes(u64::from(next), w2));
                    raw.extend(be_bytes(u64::from(generation), 2));
                }
            }
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw)?;
    Ok((widths, index.trim_end().to_string(), z.finish()?))
}

/// Write the xref section at the current position of `w`; returns the section's offset.
fn write_xref<W: Write>(
    w: &mut Counter<W>,
    kind: XrefKind,
    mut entries: BTreeMap<u32, Entry>,
    size: u32,
    xref_stream_num: u32,
    prev: Option<u64>,
    trailer: &[(Vec<u8>, Vec<u8>)],
) -> Result<u64> {
    chain_free(&mut entries);
    let at = w.pos;
    match kind {
        XrefKind::Table => {
            w.write_all(&table_bytes(&entries))?;
            let mut t = b"trailer\n<<".to_vec();
            t.extend_from_slice(format!(" /Size {size}").as_bytes());
            if let Some(p) = prev {
                t.extend_from_slice(format!(" /Prev {p}").as_bytes());
            }
            t.extend_from_slice(&trailer_dict_body(trailer, &[]));
            t.extend_from_slice(b" >>\n");
            w.write_all(&t)?;
        }
        XrefKind::Stream => {
            entries.insert(xref_stream_num, Entry::InUse { offset: at });
            let (widths, index, data) = xref_stream_data(&entries)?;
            let mut d = format!(
                "{xref_stream_num} 0 obj\n<< /Type /XRef /Size {size} /W [{} {} {}] /Index [{index}] /Filter /FlateDecode /Length {}",
                widths[0],
                widths[1],
                widths[2],
                data.len()
            )
            .into_bytes();
            if let Some(p) = prev {
                d.extend_from_slice(format!(" /Prev {p}").as_bytes());
            }
            d.extend_from_slice(&trailer_dict_body(trailer, &[]));
            d.extend_from_slice(b" >>\nstream\n");
            w.write_all(&d)?;
            w.write_all(&data)?;
            w.write_all(b"\nendstream\nendobj\n")?;
        }
    }
    w.write_all(format!("startxref\n{at}\n%%EOF\n").as_bytes())?;
    Ok(at)
}

/// Build an incremental section for `req` on top of the chain `prev`.
///
/// Nothing is written to disk. The caller appends [`Section::bytes`] to the bytes `prev` was
/// scanned from and carries [`Section::chain`] forward for the next section.
pub fn write_section(doc: &Document, prev: &ChainState, req: &SectionRequest) -> Result<Section> {
    let mut dirty = req.dirty.clone();
    dirty.sort();
    dirty.dedup();
    let mut freed: Vec<ObjId> = req
        .freed
        .iter()
        .copied()
        .filter(|f| !dirty.iter().any(|d| d.num == f.num))
        .collect();
    freed.sort();
    freed.dedup_by_key(|f| f.num);

    let crypto = Crypto::from_document(doc)?;
    let new_id = match req.new_id {
        Some(i) => i,
        None => random_id()?,
    };
    let mut w = Counter {
        inner: Vec::<u8>::new(),
        pos: prev.len,
    };
    if !prev.ends_with_eol {
        w.write_all(b"\n")?;
    }
    let mut entries = BTreeMap::new();
    let mut objects = Vec::with_capacity(dirty.len());
    for id in &dirty {
        let obj = doc.object(*id)?;
        let offset = w.pos;
        write_indirect(&mut w, *id, &obj, crypto.as_ref())?;
        entries.insert(id.num, Entry::InUse { offset });
        objects.push((*id, offset));
    }
    for f in &freed {
        entries.insert(
            f.num,
            Entry::Free {
                next: 0,
                generation: f.generation.saturating_add(1),
            },
        );
    }
    let highest = entries.keys().next_back().copied().unwrap_or(0);
    let stream_num = prev.size.max(highest + 1);
    let size = match prev.kind {
        XrefKind::Table => prev.size.max(highest + 1),
        XrefKind::Stream => stream_num + 1,
    };
    let trailer = trailer_entries(doc, IdSecond::New(new_id))?;
    let xref_at = write_xref(
        &mut w,
        prev.kind,
        entries,
        size,
        stream_num,
        Some(prev.startxref),
        &trailer,
    )?;
    let bytes = w.inner;
    Ok(Section {
        base_len: prev.len,
        objects,
        chain: ChainState {
            len: prev.len + bytes.len() as u64,
            startxref: xref_at,
            kind: prev.kind,
            size,
            sections: prev.sections + 1,
            hybrid: prev.hybrid,
            ends_with_eol: true,
            first_revision_len: prev.first_revision_len,
        },
        bytes,
        new_id,
    })
}

#[derive(Debug, Clone)]
pub struct FullWriteOptions {
    pub xref: XrefKind,
    /// Replace the second half of `/ID` (the first half is always kept: it keys R2-R4
    /// encryption).
    pub fresh_id: bool,
    /// Pin the second half of `/ID` (tests).
    pub new_id: Option<[u8; 16]>,
}

impl Default for FullWriteOptions {
    fn default() -> Self {
        FullWriteOptions {
            xref: XrefKind::Table,
            fresh_id: false,
            new_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullWriteReport {
    pub objects: usize,
    pub bytes: u64,
}

fn is_structural_stream(obj: &Object) -> Result<bool> {
    if obj.kind()? != ObjectKind::Stream {
        return Ok(false);
    }
    let t = obj.stream_dict()?.dict_get("Type")?;
    Ok(t.kind()? == ObjectKind::Name && matches!(t.name()?.as_slice(), b"ObjStm" | b"XRef"))
}

/// ID-preserving full write: every live object under its current number, one xref section, no
/// `/Prev`. Old object-stream and xref-stream containers are dropped (their members are written
/// as top-level objects).
pub fn write_full<W: Write>(
    doc: &Document,
    sink: W,
    opts: &FullWriteOptions,
    cancel: &dyn Fn() -> bool,
) -> Result<FullWriteReport> {
    let crypto = Crypto::from_document(doc)?;
    let mut w = Counter {
        inner: sink,
        pos: 0,
    };
    let mut version = doc.pdf_version()?;
    if opts.xref == XrefKind::Stream && version.as_str() < "1.5" {
        version = "1.5".into();
    }
    w.write_all(format!("%PDF-{version}\n").as_bytes())?;
    w.write_all(b"%\xE2\xE3\xCF\xD3\n")?;
    let mut ids = doc.object_ids()?;
    ids.sort();
    let mut entries = BTreeMap::new();
    let mut count = 0usize;
    let mut last_num = 0u32;
    for id in ids {
        if cancel() {
            return Err(Error::Cancelled);
        }
        let obj = doc.object(id)?;
        if is_structural_stream(&obj)? {
            continue;
        }
        let offset = w.pos;
        write_indirect(&mut w, id, &obj, crypto.as_ref())?;
        entries.insert(id.num, Entry::InUse { offset });
        last_num = last_num.max(id.num);
        count += 1;
    }
    // Numbers below the highest that were never written are free.
    for n in 1..last_num {
        entries.entry(n).or_insert(Entry::Free {
            next: 0,
            generation: 0,
        });
    }
    let second = match (opts.new_id, opts.fresh_id) {
        (Some(i), _) => IdSecond::New(i),
        (None, true) => IdSecond::New(random_id()?),
        // An unchanged pair says "same revision" to anything that cached it.
        (None, false) => IdSecond::Keep,
    };
    let trailer = trailer_entries(doc, second)?;
    let stream_num = last_num + 1;
    let size = match opts.xref {
        XrefKind::Table => last_num + 1,
        XrefKind::Stream => stream_num + 1,
    };
    write_xref(&mut w, opts.xref, entries, size, stream_num, None, &trailer)?;
    w.flush()?;
    Ok(FullWriteReport {
        objects: count,
        bytes: w.pos,
    })
}
