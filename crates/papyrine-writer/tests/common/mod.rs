#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use papyrine_cos::{
    Document, EncryptionMode, EncryptionRevision, EncryptionSpec, Fingerprint, ObjId,
    ObjectStreams, OpenOptions, Secret, StreamMode, WriteOptions,
};
use papyrine_writer::{ChainState, Section, SectionRequest, write_section};

pub const USER: &str = "user";
pub const OWNER: &str = "owner";

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A document with `pages` pages (text content streams), built with qpdf and written with the
/// given object-stream mode and encryption. `object_streams: Generate` gives an xref stream.
pub fn make_pdf(pages: usize, streams: ObjectStreams, enc: Option<EncryptionRevision>) -> Vec<u8> {
    let doc = Document::new_empty().unwrap();
    let font = doc
        .parse_object("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .unwrap();
    let font = doc.make_indirect(&font).unwrap();
    for i in 0..pages {
        let content = doc
            .new_stream(format!("BT /F1 12 Tf 72 720 Td (Page {}) Tj ET\n", i + 1).as_bytes())
            .unwrap();
        let page = doc
            .parse_object("<< /Type /Page /MediaBox [0 0 612 792] >>")
            .unwrap();
        page.dict_set("Contents", &content).unwrap();
        let res = doc.parse_object("<< /Font << >> >>").unwrap();
        res.dict_get("Font").unwrap().dict_set("F1", &font).unwrap();
        page.dict_set("Resources", &res).unwrap();
        let page = doc.make_indirect(&page).unwrap();
        doc.add_page(&page, false).unwrap();
    }
    let info = doc
        .parse_object("<< /Producer (papyrine-writer tests) /Title (Original title) >>")
        .unwrap();
    let info = doc.make_indirect(&info).unwrap();
    doc.trailer().unwrap().dict_set("Info", &info).unwrap();
    let encryption = match enc {
        None => EncryptionMode::Preserve,
        Some(r) => EncryptionMode::Encrypt(EncryptionSpec::new(r, USER, OWNER)),
    };
    doc.write(&WriteOptions {
        object_streams: streams,
        stream_data: StreamMode::Preserve,
        encryption,
        ..WriteOptions::default()
    })
    .unwrap()
    .into_vec()
}

pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Document {
    let opts = OpenOptions {
        password: password.map(Secret::from),
        ..OpenOptions::default()
    };
    Document::open_bytes(bytes, &opts).unwrap()
}

pub fn fingerprints(doc: &Document, ids: &[ObjId]) -> BTreeMap<ObjId, Fingerprint> {
    ids.iter()
        .map(|id| (*id, doc.fingerprint(&doc.object(*id).unwrap()).unwrap()))
        .collect()
}

pub fn all_fingerprints(doc: &Document) -> BTreeMap<ObjId, Fingerprint> {
    fingerprints(doc, &doc.object_ids().unwrap())
}

/// Fingerprint with the stream `/Length` removed: it is a computed value (on disk it is the
/// encrypted length, in memory the plain length).
pub fn normalized(mut f: Fingerprint) -> Fingerprint {
    if f.is_stream {
        let t = String::from_utf8_lossy(&f.repr).into_owned();
        if let Some(i) = t.find("/Length ") {
            let rest = &t[i + 8..];
            let mut toks = rest.split(' ');
            let first = toks.next().unwrap_or("");
            let skip = if toks
                .next()
                .is_some_and(|g| g.chars().all(|c| c.is_ascii_digit()))
                && rest[first.len()..].trim_start().split(' ').nth(1) == Some("R")
            {
                first.len() + 1 + rest[first.len() + 1..].find('R').unwrap() + 1
            } else {
                first.len()
            };
            let mut out = t[..i].trim_end().to_string();
            out.push(' ');
            out.push_str(rest[skip..].trim_start());
            f.repr = out.into_bytes();
        }
    }
    f
}

/// Compare `ids` between the in-memory `doc` and the re-parse of `bytes`.
pub fn assert_parity(
    doc: &Document,
    bytes: Vec<u8>,
    password: Option<&str>,
    ids: &[ObjId],
) -> Document {
    let re = open(bytes, password);
    let log = re.repair_log();
    assert!(
        log.is_empty(),
        "re-parse needed repairs: {:?}",
        log.entries()
    );
    for id in ids {
        let a = normalized(doc.fingerprint(&doc.object(*id).unwrap()).unwrap());
        let b = normalized(re.fingerprint(&re.object(*id).unwrap()).unwrap());
        assert_eq!(
            a,
            b,
            "object {id} differs\n mem: {}\n disk: {}",
            String::from_utf8_lossy(&a.repr),
            String::from_utf8_lossy(&b.repr)
        );
    }
    re
}

pub fn append(original: &[u8], section: &Section) -> Vec<u8> {
    assert_eq!(section.base_len, original.len() as u64);
    let mut v = original.to_vec();
    v.extend_from_slice(&section.bytes);
    v
}

pub fn incremental(
    doc: &Document,
    original: &[u8],
    dirty: &[ObjId],
    freed: &[ObjId],
) -> (Vec<u8>, Section) {
    let chain = ChainState::scan(original).unwrap();
    let s = write_section(
        doc,
        &chain,
        &SectionRequest {
            dirty: dirty.to_vec(),
            freed: freed.to_vec(),
            new_id: None,
        },
    )
    .unwrap();
    (append(original, &s), s)
}

/// `qpdf --check` exit status: 0 clean, 3 warnings, 2 errors.
pub fn qpdf_check(path: &Path, password: Option<&str>) -> (i32, String) {
    let mut c = Command::new("qpdf");
    c.arg("--check");
    if let Some(p) = password {
        c.arg(format!("--password={p}"));
    }
    c.arg(path);
    let out = c.output().expect("qpdf CLI available");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

pub fn write_tmp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// xorshift64*: deterministic, dependency-free.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}
