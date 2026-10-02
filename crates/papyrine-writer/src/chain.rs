//! Reading the tail of an existing file: where the last cross-reference section is, what kind it
//! is, how many sections the `/Prev` chain has, and the current `/Size`. This is everything an
//! incremental update needs from the bytes it appends to.

use std::collections::HashSet;
use std::fs::File;
use std::io;

use crate::{Error, Result};

/// Random-access read, so multi-hundred-megabyte files are never loaded to scan their tail.
pub trait ReadAt {
    fn len(&self) -> io::Result<u64>;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    fn is_empty(&self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Fill as much of `buf` as the file has at `offset`; returns the bytes read.
    fn read_up_to(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            let r = self.read_at(offset + n as u64, &mut buf[n..])?;
            if r == 0 {
                break;
            }
            n += r;
        }
        Ok(n)
    }
}

impl ReadAt for [u8] {
    fn len(&self) -> io::Result<u64> {
        Ok(<[u8]>::len(self) as u64)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Some(rest) = self.get(offset as usize..) else {
            return Ok(0);
        };
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }
}

impl ReadAt for Vec<u8> {
    fn len(&self) -> io::Result<u64> {
        Ok(Vec::len(self) as u64)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.as_slice().read_at(offset, buf)
    }
}

impl ReadAt for File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(self, buf, offset)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(self, buf, offset)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefKind {
    Table,
    Stream,
}

/// The end of a byte sequence that is a valid PDF, as far as appending is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainState {
    /// Total length of the bytes so far (base plus appended sections).
    pub len: u64,
    /// Offset of the newest cross-reference section.
    pub startxref: u64,
    /// Form of the newest section; a new section copies it.
    pub kind: XrefKind,
    /// `/Size` of the newest trailer.
    pub size: u32,
    /// Number of cross-reference sections in the `/Prev` chain.
    pub sections: u32,
    /// Some section carried an `/XRefStm` (a hybrid-reference file).
    pub hybrid: bool,
    /// The last byte is an EOL, so a section can start right away.
    pub ends_with_eol: bool,
    /// Approximate length of the first revision (through its `%%EOF`), for the save policy.
    pub first_revision_len: Option<u64>,
}

const TAIL: usize = 2048;
const MAX_SECTIONS: u32 = 100_000;
const DICT_WINDOW: usize = 64 * 1024;

fn bad(msg: impl Into<String>) -> Error {
    Error::Chain(msg.into())
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn skip_ws(b: &[u8], mut p: usize) -> usize {
    while p < b.len() {
        if is_ws(b[p]) {
            p += 1;
        } else if b[p] == b'%' {
            while p < b.len() && b[p] != b'\n' && b[p] != b'\r' {
                p += 1;
            }
        } else {
            break;
        }
    }
    p
}

fn token_end(b: &[u8], mut p: usize) -> usize {
    while p < b.len() && !is_ws(b[p]) && !is_delim(b[p]) {
        p += 1;
    }
    p
}

fn parse_uint(b: &[u8], p: usize) -> Option<(u64, usize)> {
    let e = token_end(b, p);
    if e == p || !b[p..e].iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(&b[p..e])
        .ok()?
        .parse()
        .ok()
        .map(|v| (v, e))
}

/// End of the object starting at `p` (one past its last byte), or `None` if malformed.
fn skip_object(b: &[u8], p: usize, depth: u32) -> Option<usize> {
    if depth > 64 {
        return None;
    }
    let p = skip_ws(b, p);
    match *b.get(p)? {
        b'<' if b.get(p + 1) == Some(&b'<') => {
            let mut q = p + 2;
            loop {
                q = skip_ws(b, q);
                if b.get(q) == Some(&b'>') && b.get(q + 1) == Some(&b'>') {
                    return Some(q + 2);
                }
                q = skip_object(b, q, depth + 1)?; // key
                q = skip_object(b, q, depth + 1)?; // value
            }
        }
        b'<' => Some(p + 1 + b[p + 1..].iter().position(|&c| c == b'>')? + 1),
        b'[' => {
            let mut q = p + 1;
            loop {
                q = skip_ws(b, q);
                if *b.get(q)? == b']' {
                    return Some(q + 1);
                }
                q = skip_object(b, q, depth + 1)?;
            }
        }
        b'(' => {
            let (mut q, mut level) = (p + 1, 1u32);
            while q < b.len() {
                match b[q] {
                    b'\\' => q += 1,
                    b'(' => level += 1,
                    b')' => {
                        level -= 1;
                        if level == 0 {
                            return Some(q + 1);
                        }
                    }
                    _ => {}
                }
                q += 1;
            }
            None
        }
        b'/' => Some(token_end(b, p + 1)),
        _ => {
            let e = token_end(b, p);
            if e == p {
                return None;
            }
            // `n g R` reference?
            if let Some((_, e1)) = parse_uint(b, p) {
                let q = skip_ws(b, e1);
                if let Some((_, e2)) = parse_uint(b, q) {
                    let r = skip_ws(b, e2);
                    if b.get(r) == Some(&b'R') && token_end(b, r) == r + 1 {
                        return Some(r + 1);
                    }
                }
            }
            Some(e)
        }
    }
}

/// Raw value slices for the top-level keys of the dictionary at `p` (which must be `<<`).
type KeySpan = (Vec<u8>, (usize, usize));

fn top_level_keys(b: &[u8], p: usize) -> Option<Vec<KeySpan>> {
    let mut q = skip_ws(b, p);
    if b.get(q..q + 2)? != b"<<" {
        return None;
    }
    q += 2;
    let mut out = Vec::new();
    loop {
        q = skip_ws(b, q);
        if b.get(q..q + 2)? == b">>" {
            return Some(out);
        }
        if *b.get(q)? != b'/' {
            return None;
        }
        let ke = token_end(b, q + 1);
        let key = b[q + 1..ke].to_vec();
        let vs = skip_ws(b, ke);
        let ve = skip_object(b, vs, 0)?;
        out.push((key, (vs, ve)));
        q = ve;
    }
}

struct Section {
    kind: XrefKind,
    size: Option<u64>,
    prev: Option<u64>,
    xrefstm: bool,
}

fn read_section(src: &(impl ReadAt + ?Sized), off: u64, len: u64) -> Result<Section> {
    if off >= len {
        return Err(bad(format!(
            "xref offset {off} is beyond the end of the file"
        )));
    }
    let mut head = vec![0u8; 4096];
    let n = src.read_up_to(off, &mut head)?;
    head.truncate(n);
    let p = skip_ws(&head, 0);
    let (kind, dict_buf, dict_at) = if head[p..].starts_with(b"xref") {
        // Entries are fixed-width lines; "trailer" cannot occur inside them.
        let mut carry_start = off + p as u64 + 4;
        let mut next = carry_start;
        let mut carry: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 256 * 1024];
        let found = loop {
            let n = src.read_up_to(next, &mut chunk)?;
            if n == 0 {
                return Err(bad("xref table has no trailer"));
            }
            carry.extend_from_slice(&chunk[..n]);
            next += n as u64;
            if let Some(i) = find(&carry, b"trailer") {
                break carry_start + i as u64;
            }
            // Keep a tail so a keyword split across chunks is still found.
            let keep = carry.len().saturating_sub(7);
            carry_start += keep as u64;
            carry.drain(..keep);
        };
        let mut win = vec![0u8; DICT_WINDOW];
        let n = src.read_up_to(found + 7, &mut win)?;
        win.truncate(n);
        (XrefKind::Table, win, 0usize)
    } else {
        // `N G obj << ... >> stream`
        let (_, e1) =
            parse_uint(&head, p).ok_or_else(|| bad("startxref does not point at an xref"))?;
        let q = skip_ws(&head, e1);
        let (_, e2) =
            parse_uint(&head, q).ok_or_else(|| bad("startxref does not point at an xref"))?;
        let q = skip_ws(&head, e2);
        if !head[q..].starts_with(b"obj") {
            return Err(bad("startxref does not point at an xref"));
        }
        let mut win = vec![0u8; DICT_WINDOW];
        let n = src.read_up_to(off + q as u64 + 3, &mut win)?;
        win.truncate(n);
        (XrefKind::Stream, win, 0usize)
    };
    let keys =
        top_level_keys(&dict_buf, dict_at).ok_or_else(|| bad("unreadable trailer dictionary"))?;
    let get = |k: &[u8]| keys.iter().find(|(n, _)| n == k).map(|(_, r)| *r);
    if kind == XrefKind::Stream {
        let ty = get(b"Type").map(|(s, e)| &dict_buf[s..e]);
        if ty != Some(b"/XRef") {
            return Err(bad("xref stream object is not /Type /XRef"));
        }
    }
    let int = |k: &[u8]| {
        get(k).and_then(|(s, e)| {
            std::str::from_utf8(&dict_buf[s..e])
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        })
    };
    Ok(Section {
        kind,
        size: int(b"Size"),
        prev: int(b"Prev"),
        xrefstm: get(b"XRefStm").is_some(),
    })
}

impl ChainState {
    /// Scan `src` (the whole file, or base plus sections) and describe its newest section.
    /// Fails with [`Error::Chain`] when the file would need repair before it can be extended:
    /// no `%PDF-` at byte 0, no readable `startxref`, an offset that is not an xref, or a loop.
    pub fn scan(src: &(impl ReadAt + ?Sized)) -> Result<ChainState> {
        let len = src.len()?;
        let mut head = [0u8; 16];
        let n = src.read_up_to(0, &mut head)?;
        if !head[..n].starts_with(b"%PDF-") {
            return Err(bad("no %PDF- header at byte 0"));
        }
        let tail_len = (len as usize).min(TAIL);
        let mut tail = vec![0u8; tail_len];
        let n = src.read_up_to(len - tail_len as u64, &mut tail)?;
        tail.truncate(n);
        let sx = rfind(&tail, b"startxref").ok_or_else(|| bad("no startxref in the last 2 KiB"))?;
        let p = skip_ws(&tail, sx + 9);
        let (startxref, _) = parse_uint(&tail, p).ok_or_else(|| bad("startxref has no offset"))?;
        let ends_with_eol = matches!(tail.last(), Some(b'\n' | b'\r'));

        let mut seen = HashSet::new();
        let mut off = startxref;
        let mut sections = 0u32;
        let mut hybrid = false;
        let (mut kind, mut size) = (None, None);
        loop {
            if !seen.insert(off) {
                return Err(bad("/Prev chain loops"));
            }
            sections += 1;
            if sections > MAX_SECTIONS {
                return Err(bad("/Prev chain is absurdly long"));
            }
            let s = read_section(src, off, len)?;
            kind.get_or_insert(s.kind);
            if size.is_none() {
                size = s.size;
            }
            hybrid |= s.xrefstm;
            match s.prev {
                Some(p) => off = p,
                None => break,
            }
        }
        let size = size.ok_or_else(|| bad("trailer has no /Size"))?;
        Ok(ChainState {
            len,
            startxref,
            kind: kind.expect("at least one section"),
            size: u32::try_from(size).map_err(|_| bad("/Size too large"))?,
            sections,
            hybrid,
            ends_with_eol,
            first_revision_len: first_revision_len(src, off, len),
        })
    }
}

/// Offset just past the first `%%EOF` that follows the oldest xref section.
fn first_revision_len(src: &(impl ReadAt + ?Sized), oldest: u64, len: u64) -> Option<u64> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut pos = oldest;
    let mut carry: Vec<u8> = Vec::new();
    while pos < len {
        let n = src.read_up_to(pos, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        let start = pos - carry.len() as u64;
        carry.extend_from_slice(&buf[..n]);
        if let Some(i) = find(&carry, b"%%EOF") {
            let end = start + i as u64 + 5;
            return Some(end.min(len));
        }
        let keep = carry.len().saturating_sub(4);
        carry.drain(..keep);
        pos += n as u64;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\nxref\n0 2\n0000000000 65535 f \n0000000009 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R /ID [<aa> (b)] >>\nstartxref\n49\n%%EOF\n".to_vec()
    }

    #[test]
    fn scans_a_table_file() {
        let pdf = tiny();
        let off = find(&pdf, b"xref\n0 2").unwrap();
        let mut pdf = pdf;
        let s = String::from_utf8(pdf.clone())
            .unwrap()
            .replace("\n49\n", &format!("\n{off}\n"));
        pdf = s.into_bytes();
        let c = ChainState::scan(&pdf).unwrap();
        assert_eq!((c.kind, c.size, c.sections), (XrefKind::Table, 2, 1));
        assert!(c.ends_with_eol && !c.hybrid);
        assert_eq!(c.first_revision_len, Some(pdf.len() as u64 - 1));
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            ChainState::scan(&b"hello"[..]),
            Err(Error::Chain(_))
        ));
        assert!(matches!(
            ChainState::scan(&b"%PDF-1.4\nstartxref\n5\n%%EOF\n"[..]),
            Err(Error::Chain(_))
        ));
        assert!(matches!(
            ChainState::scan(&b"%PDF-1.4\nstartxref\n99999\n%%EOF\n"[..]),
            Err(Error::Chain(_))
        ));
    }

    #[test]
    fn dict_key_scanner() {
        let d = b"<< /A [1 2 (x)] /B << /Prev 3 >> /Prev 12 /R 4 0 R /S <ab> >>";
        let k = top_level_keys(d, 0).unwrap();
        let names: Vec<_> = k
            .iter()
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
            .collect();
        assert_eq!(names, ["A", "B", "Prev", "R", "S"]);
        let r = k[3].1;
        assert_eq!(&d[r.0..r.1], b"4 0 R");
    }
}
