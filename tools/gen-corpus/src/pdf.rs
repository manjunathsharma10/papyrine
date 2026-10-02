//! Minimal streaming PDF writer: classic xref tables, object streams, xref streams.

use miniz_oxide::deflate::compress_to_vec_zlib;
use std::io::{self, Write};

pub fn flate(data: &[u8], level: u8) -> Vec<u8> {
    compress_to_vec_zlib(data, level)
}

pub struct Out<W: Write> {
    w: W,
    pub pos: u64,
    /// Byte offset per object number (0 = not written as a top-level object).
    offsets: Vec<u64>,
    /// (object number, containing object stream number, index) for packed objects.
    packed_ref: Vec<(u32, u32, u32)>,
    pending: Vec<(u32, Vec<u8>)>,
    next_stm: u32,
    pack_size: usize,
}

impl<W: Write> Out<W> {
    pub fn new(w: W) -> Self {
        Out {
            w,
            pos: 0,
            offsets: vec![0],
            packed_ref: Vec::new(),
            pending: Vec::new(),
            next_stm: 0,
            pack_size: 100,
        }
    }

    pub fn raw(&mut self, b: &[u8]) -> io::Result<()> {
        self.w.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }

    pub fn header(&mut self, version: &str) -> io::Result<()> {
        self.raw(format!("%PDF-{version}\n").as_bytes())?;
        self.raw(b"%\xe2\xe3\xcf\xd3\n")
    }

    fn mark(&mut self, num: u32) {
        let n = num as usize;
        if self.offsets.len() <= n {
            self.offsets.resize(n + 1, 0);
        }
        self.offsets[n] = self.pos;
    }

    pub fn obj(&mut self, num: u32, body: &[u8]) -> io::Result<()> {
        self.mark(num);
        self.raw(format!("{num} 0 obj\n").as_bytes())?;
        self.raw(body)?;
        self.raw(b"\nendobj\n")
    }

    /// `dict` holds the dictionary entries without `<<`/`>>` and without /Length.
    pub fn stream(&mut self, num: u32, dict: &str, data: &[u8]) -> io::Result<()> {
        self.mark(num);
        let dict = dict.trim();
        let sep = if dict.is_empty() { "" } else { " " };
        self.raw(
            format!(
                "{num} 0 obj\n<< {dict}{sep}/Length {} >>\nstream\n",
                data.len()
            )
            .as_bytes(),
        )?;
        self.raw(data)?;
        self.raw(b"\nendstream\nendobj\n")
    }

    pub fn flate_stream(&mut self, num: u32, dict: &str, raw: &[u8], level: u8) -> io::Result<()> {
        let z = flate(raw, level);
        self.stream(num, &format!("{dict} /Filter /FlateDecode"), &z)
    }

    /// Queue an object for packing into an object stream (flushes full groups).
    pub fn packed(&mut self, num: u32, body: Vec<u8>) -> io::Result<()> {
        self.pending.push((num, body));
        if self.pending.len() >= self.pack_size {
            self.flush_pack()?;
        }
        Ok(())
    }

    pub fn set_object_stream_numbers(&mut self, first: u32, group: usize) {
        self.next_stm = first;
        self.pack_size = group;
    }

    pub fn flush_pack(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let items = std::mem::take(&mut self.pending);
        let stm = self.next_stm;
        self.next_stm += 1;
        let mut head = String::new();
        let mut body = Vec::new();
        for (i, (num, b)) in items.iter().enumerate() {
            head.push_str(&format!("{num} {} ", body.len()));
            body.extend_from_slice(b);
            body.push(b'\n');
            self.packed_ref.push((*num, stm, i as u32));
        }
        let mut data = head.into_bytes();
        let first = data.len();
        data.extend_from_slice(&body);
        let dict = format!("/Type /ObjStm /N {} /First {first}", items.len());
        self.flate_stream(stm, &dict, &data, 6)
    }

    fn size(&self) -> usize {
        let m = self
            .packed_ref
            .iter()
            .map(|p| p.0 as usize)
            .max()
            .unwrap_or(0);
        self.offsets.len().max(m + 1)
    }

    /// Classic xref table and trailer. `extra` is appended inside the trailer dictionary.
    pub fn finish_classic(&mut self, root: u32, extra: &str) -> io::Result<u64> {
        let xref_pos = self.pos;
        let n = self.offsets.len();
        let mut s = String::with_capacity(n * 20 + 64);
        s.push_str(&format!("xref\n0 {n}\n"));
        s.push_str("0000000000 65535 f \n");
        for i in 1..n {
            let off = self.offsets[i];
            if off == 0 {
                s.push_str("0000000000 00000 f \n");
            } else {
                s.push_str(&format!("{off:010} 00000 n \n"));
            }
        }
        s.push_str(&format!(
            "trailer\n<< /Size {n} /Root {root} 0 R {extra} >>\nstartxref\n{xref_pos}\n%%EOF\n"
        ));
        self.raw(s.as_bytes())?;
        Ok(xref_pos)
    }

    /// Flushes packed objects and writes a cross-reference stream as object `xref_num`.
    pub fn finish_xref_stream(&mut self, xref_num: u32, root: u32, extra: &str) -> io::Result<u64> {
        self.flush_pack()?;
        let xref_pos = self.pos;
        self.mark(xref_num);
        let n = self.size();
        let mut packed = vec![(0u32, 0u32); n];
        for &(num, stm, idx) in &self.packed_ref {
            packed[num as usize] = (stm, idx);
        }
        let mut data = Vec::with_capacity(n * 7);
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let off = self.offsets.get(i).copied().unwrap_or(0);
            if i == 0 {
                data.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff]);
            } else if off != 0 {
                data.push(1);
                data.extend_from_slice(&(off as u32).to_be_bytes());
                data.extend_from_slice(&[0, 0]);
            } else if packed[i].0 != 0 {
                data.push(2);
                data.extend_from_slice(&packed[i].0.to_be_bytes());
                data.extend_from_slice(&(packed[i].1 as u16).to_be_bytes());
            } else {
                data.extend_from_slice(&[0; 7]);
            }
        }
        let dict = format!("/Type /XRef /Size {n} /W [1 4 2] /Root {root} 0 R {extra}");
        self.flate_stream(xref_num, &dict, &data, 6)?;
        self.raw(format!("startxref\n{xref_pos}\n%%EOF\n").as_bytes())?;
        Ok(xref_pos)
    }

    pub fn into_inner(self) -> W {
        self.w
    }
}

/// PDF literal-string escaping for ASCII text.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '(' | ')' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_xref_offsets_point_at_objects() {
        let mut o = Out::new(Vec::new());
        o.obj(1, b"<< /Type /Catalog >>").unwrap();
        o.stream(3, "", b"hello").unwrap();
        let xref = o.finish_classic(1, "").unwrap() as usize;
        let b = o.into_inner();
        let text = String::from_utf8(b).unwrap();
        let table: Vec<&str> = text[xref..].lines().skip(2).take(4).collect();
        assert_eq!(table[0], "0000000000 65535 f ");
        assert_eq!(table[2], "0000000000 00000 f "); // object 2 unused
        for (n, line) in [(1usize, table[1]), (3, table[3])] {
            let off: usize = line[..10].parse().unwrap();
            assert!(text[off..].starts_with(&format!("{n} 0 obj")));
        }
        assert!(text.ends_with("%%EOF\n"));
    }

    #[test]
    fn stream_length_matches_payload() {
        let mut o = Out::new(Vec::new());
        o.stream(1, "/Foo /Bar", b"12345").unwrap();
        let s = String::from_utf8(o.into_inner()).unwrap();
        assert!(s.contains("<< /Foo /Bar /Length 5 >>\nstream\n12345\nendstream"));
    }
}
