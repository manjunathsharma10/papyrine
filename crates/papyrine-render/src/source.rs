//! Multi-buffer byte source: base file (usually an mmap) followed by appended
//! incremental-update sections. PDFium reads it through `FPDF_LoadCustomDocument`,
//! so the bytes are never copied into one contiguous heap buffer.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// Shared immutable bytes: an mmap, a `Vec<u8>`, or anything `AsRef<[u8]>`.
pub type Bytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

pub fn bytes_from_vec(v: Vec<u8>) -> Bytes {
    Arc::new(v)
}

/// Memory-map a file read-only.
///
/// The file must not be truncated or modified while mapped (a shrinking file
/// raises SIGBUS on access). The host owns that contract; the render snapshot
/// files are write-once.
pub fn open_mmap(path: &Path) -> io::Result<Bytes> {
    let file = File::open(path)?;
    // SAFETY: see the contract above; the mapping is read-only.
    let map = unsafe { memmap2::Mmap::map(&file)? };
    Ok(Arc::new(map))
}

/// `base ‖ section₁ ‖ … ‖ sectionₖ`, addressed as one flat file.
#[derive(Clone)]
pub struct MultiBuf {
    parts: Vec<Bytes>,
    /// `starts[i]` is the absolute offset of `parts[i]`.
    starts: Vec<u64>,
    len: u64,
}

impl MultiBuf {
    pub fn new(base: Bytes) -> Self {
        Self::from_parts(std::iter::once(base))
    }

    pub fn from_parts(parts: impl IntoIterator<Item = Bytes>) -> Self {
        let mut m = MultiBuf {
            parts: Vec::new(),
            starts: Vec::new(),
            len: 0,
        };
        for p in parts {
            m.push(p);
        }
        m
    }

    /// Append one section (empty buffers are ignored).
    pub fn push(&mut self, part: Bytes) {
        let n = (*part).as_ref().len() as u64;
        if n == 0 {
            return;
        }
        self.starts.push(self.len);
        self.parts.push(part);
        self.len += n;
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of buffers (base plus sections).
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    pub fn base(&self) -> Option<&Bytes> {
        self.parts.first()
    }

    /// Fill `out` from absolute offset `pos`. Returns false if out of range.
    pub fn read_at(&self, pos: u64, out: &mut [u8]) -> bool {
        let Some(end) = pos.checked_add(out.len() as u64) else {
            return false;
        };
        if end > self.len {
            return false;
        }
        let mut idx = self.starts.partition_point(|&s| s <= pos).saturating_sub(1);
        let mut pos = pos;
        let mut done = 0;
        while done < out.len() {
            let part: &[u8] = (*self.parts[idx]).as_ref();
            let off = (pos - self.starts[idx]) as usize;
            let n = (part.len() - off).min(out.len() - done);
            out[done..done + n].copy_from_slice(&part[off..off + n]);
            done += n;
            pos += n as u64;
            idx += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_parts() {
        let m = MultiBuf::from_parts([
            bytes_from_vec(b"abcd".to_vec()),
            bytes_from_vec(Vec::new()),
            bytes_from_vec(b"efg".to_vec()),
            bytes_from_vec(b"hij".to_vec()),
        ]);
        assert_eq!(m.len(), 10);
        assert_eq!(m.part_count(), 3);
        let mut b = [0u8; 6];
        assert!(m.read_at(2, &mut b));
        assert_eq!(&b, b"cdefgh");
        let mut all = [0u8; 10];
        assert!(m.read_at(0, &mut all));
        assert_eq!(&all, b"abcdefghij");
        assert!(!m.read_at(8, &mut [0u8; 3]));
        assert!(m.read_at(10, &mut []));
    }
}
