//! On-disk record format.
//!
//! File: 8-byte magic, then records `[len u32 LE][crc32c(payload) u32 LE][payload]` where
//! payload = `[kind u8][body]`. All integers little-endian.

use crate::error::{JournalError, Result};

pub const MAGIC: &[u8; 8] = b"PJNL\0\0\0\x01";
/// Anything bigger is treated as garbage length (torn or corrupt tail).
pub const MAX_RECORD: usize = 1 << 30;

pub type Hash = [u8; 32];

/// Opaque object identity (PDF object number + generation) for after-images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId {
    pub num: u32,
    pub generation: u16,
}

/// External input stored in `blobs/<blake3 hex>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRef {
    pub hash: Hash,
    pub len: u64,
}

/// Serialized object (+ stream bytes) after a command. Opaque to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfterImage {
    pub id: ObjId,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Intent {
        seq: u64,
        ts_ms: u64,
        command: String,
        params: serde_json::Value,
        blobs: Vec<BlobRef>,
    },
    Commit {
        seq: u64,
        ts_ms: u64,
        after_images: Vec<AfterImage>,
        created: Vec<ObjId>,
        digest: Hash,
    },
    /// Everything up to and including `upto_seq` is contained in blob `blob`.
    Checkpoint {
        upto_seq: u64,
        ts_ms: u64,
        blob: BlobRef,
    },
    /// Document saved or closed cleanly: everything before this record is moot.
    Clean { ts_ms: u64 },
    /// The Intent `seq` was resolved without a Commit (failed cleanly, or skipped/redone
    /// after a crash; `crashed` intents count towards quarantine).
    Abandon { seq: u64, ts_ms: u64, crashed: bool },
}

const K_INTENT: u8 = 1;
const K_COMMIT: u8 = 2;
const K_CHECKPOINT: u8 = 3;
const K_CLEAN: u8 = 4;
const K_ABANDON: u8 = 5;

/// BLAKE3 over the canonical (uncompressed) after-image set and created ids.
pub fn commit_digest(images: &[AfterImage], created: &[ObjId]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&(images.len() as u64).to_le_bytes());
    for im in images {
        h.update(&im.id.num.to_le_bytes());
        h.update(&im.id.generation.to_le_bytes());
        h.update(&(im.data.len() as u64).to_le_bytes());
        h.update(&im.data);
    }
    h.update(&(created.len() as u64).to_le_bytes());
    for c in created {
        h.update(&c.num.to_le_bytes());
        h.update(&c.generation.to_le_bytes());
    }
    *h.finalize().as_bytes()
}

pub fn hash_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn id(&mut self, id: ObjId) {
        self.u32(id.num);
        self.u16(id.generation);
    }
}

struct R<'a>(&'a [u8]);
type Rd<T> = std::result::Result<T, ()>;
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Rd<&'a [u8]> {
        if self.0.len() < n {
            return Err(());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Rd<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Rd<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Rd<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Rd<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Rd<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn hash(&mut self) -> Rd<Hash> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn id(&mut self) -> Rd<ObjId> {
        Ok(ObjId {
            num: self.u32()?,
            generation: self.u16()?,
        })
    }
}

/// Tuning for after-image compression.
#[derive(Debug, Clone, Copy)]
pub struct Compression {
    pub threshold: usize,
    pub level: i32,
}

/// Serialize a record to its framed on-disk form.
pub fn encode(rec: &Record, comp: Compression) -> Result<Vec<u8>> {
    let mut w = W(Vec::with_capacity(256));
    match rec {
        Record::Intent {
            seq,
            ts_ms,
            command,
            params,
            blobs,
        } => {
            w.u8(K_INTENT);
            w.u64(*seq);
            w.u64(*ts_ms);
            w.bytes(command.as_bytes());
            w.bytes(serde_json::to_string(params)?.as_bytes());
            w.u32(blobs.len() as u32);
            for b in blobs {
                w.0.extend_from_slice(&b.hash);
                w.u64(b.len);
            }
        }
        Record::Commit {
            seq,
            ts_ms,
            after_images,
            created,
            digest,
        } => {
            w.u8(K_COMMIT);
            w.u64(*seq);
            w.u64(*ts_ms);
            w.0.extend_from_slice(digest);
            w.u32(created.len() as u32);
            for c in created {
                w.id(*c);
            }
            w.u32(after_images.len() as u32);
            for im in after_images {
                w.id(im.id);
                let packed = (im.data.len() > comp.threshold)
                    .then(|| zstd::bulk::compress(&im.data, comp.level).ok())
                    .flatten()
                    .filter(|c| c.len() < im.data.len());
                match packed {
                    Some(c) => {
                        w.u8(1);
                        w.u64(im.data.len() as u64);
                        w.bytes(&c);
                    }
                    None => {
                        w.u8(0);
                        w.u64(im.data.len() as u64);
                        w.bytes(&im.data);
                    }
                }
            }
        }
        Record::Checkpoint {
            upto_seq,
            ts_ms,
            blob,
        } => {
            w.u8(K_CHECKPOINT);
            w.u64(*upto_seq);
            w.u64(*ts_ms);
            w.0.extend_from_slice(&blob.hash);
            w.u64(blob.len);
        }
        Record::Clean { ts_ms } => {
            w.u8(K_CLEAN);
            w.u64(*ts_ms);
        }
        Record::Abandon {
            seq,
            ts_ms,
            crashed,
        } => {
            w.u8(K_ABANDON);
            w.u64(*seq);
            w.u64(*ts_ms);
            w.u8(*crashed as u8);
        }
    }
    let payload = w.0;
    if payload.len() > MAX_RECORD {
        return Err(JournalError::Corrupt("record exceeds 1 GiB".into()));
    }
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32c::crc32c(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Why decoding stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Clean end of file.
    End,
    /// Header or payload cut short (torn write).
    Truncated,
    /// Length absurd, CRC mismatch, or a body that does not parse / verify.
    Corrupt,
}

/// Decode the record starting at `buf[0]`. Returns the record and the framed length.
pub fn decode(buf: &[u8]) -> std::result::Result<(Record, usize), Stop> {
    if buf.is_empty() {
        return Err(Stop::End);
    }
    if buf.len() < 8 {
        return Err(Stop::Truncated);
    }
    let len = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    if len == 0 || len > MAX_RECORD {
        return Err(Stop::Corrupt);
    }
    if buf.len() < 8 + len {
        return Err(Stop::Truncated);
    }
    let payload = &buf[8..8 + len];
    if crc32c::crc32c(payload) != crc {
        return Err(Stop::Corrupt);
    }
    let rec = parse(payload).map_err(|()| Stop::Corrupt)?;
    Ok((rec, 8 + len))
}

fn parse(p: &[u8]) -> Rd<Record> {
    let mut r = R(p);
    let rec = match r.u8()? {
        K_INTENT => {
            let seq = r.u64()?;
            let ts_ms = r.u64()?;
            let command = String::from_utf8(r.bytes()?.to_vec()).map_err(|_| ())?;
            let params = serde_json::from_slice(r.bytes()?).map_err(|_| ())?;
            let n = r.u32()? as usize;
            let mut blobs = Vec::new();
            for _ in 0..n {
                blobs.push(BlobRef {
                    hash: r.hash()?,
                    len: r.u64()?,
                });
            }
            Record::Intent {
                seq,
                ts_ms,
                command,
                params,
                blobs,
            }
        }
        K_COMMIT => {
            let seq = r.u64()?;
            let ts_ms = r.u64()?;
            let digest = r.hash()?;
            let nc = r.u32()? as usize;
            let mut created = Vec::new();
            for _ in 0..nc {
                created.push(r.id()?);
            }
            let ni = r.u32()? as usize;
            let mut after_images = Vec::new();
            for _ in 0..ni {
                let id = r.id()?;
                let flags = r.u8()?;
                let raw_len = r.u64()? as usize;
                let stored = r.bytes()?;
                let data = if flags & 1 != 0 {
                    if raw_len > MAX_RECORD {
                        return Err(());
                    }
                    zstd::bulk::decompress(stored, raw_len).map_err(|_| ())?
                } else {
                    stored.to_vec()
                };
                if data.len() != raw_len {
                    return Err(());
                }
                after_images.push(AfterImage { id, data });
            }
            if commit_digest(&after_images, &created) != digest {
                return Err(());
            }
            Record::Commit {
                seq,
                ts_ms,
                after_images,
                created,
                digest,
            }
        }
        K_CHECKPOINT => Record::Checkpoint {
            upto_seq: r.u64()?,
            ts_ms: r.u64()?,
            blob: BlobRef {
                hash: r.hash()?,
                len: r.u64()?,
            },
        },
        K_CLEAN => Record::Clean { ts_ms: r.u64()? },
        K_ABANDON => Record::Abandon {
            seq: r.u64()?,
            ts_ms: r.u64()?,
            crashed: r.u8()? != 0,
        },
        _ => return Err(()),
    };
    if !r.0.is_empty() {
        return Err(());
    }
    Ok(rec)
}

/// Result of scanning a whole log image.
#[derive(Debug)]
pub struct Scan {
    pub records: Vec<Record>,
    /// Byte length of the valid prefix (header + all good records).
    pub valid_len: u64,
    /// Why scanning stopped.
    pub stop: Stop,
}

pub fn scan(buf: &[u8]) -> Result<Scan> {
    if buf.len() < MAGIC.len() {
        // Header itself torn: only a prefix of the magic is acceptable.
        if MAGIC.starts_with(buf) {
            return Ok(Scan {
                records: vec![],
                valid_len: 0,
                stop: Stop::Truncated,
            });
        }
        return Err(JournalError::Corrupt("bad journal magic".into()));
    }
    if &buf[..8] != MAGIC {
        return Err(JournalError::Corrupt(
            "bad journal magic or unsupported version".into(),
        ));
    }
    let mut pos = 8;
    let mut records = Vec::new();
    let stop = loop {
        match decode(&buf[pos..]) {
            Ok((rec, n)) => {
                records.push(rec);
                pos += n;
            }
            Err(s) => break s,
        }
    };
    Ok(Scan {
        records,
        valid_len: pos as u64,
        stop,
    })
}
