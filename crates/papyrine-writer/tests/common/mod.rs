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

/// `qpdf --check` exit status: 0 clean, 3 warnings, 2 errors. The CLI is a test-time tool; when
/// it is not installed the check passes vacuously (the library re-parse checks still run).
pub fn qpdf_check(path: &Path, password: Option<&str>) -> (i32, String) {
    let mut c = Command::new("qpdf");
    c.arg("--check");
    if let Some(p) = password {
        c.arg(format!("--password={p}"));
    }
    c.arg(path);
    let Ok(out) = c.output() else {
        eprintln!("qpdf CLI not installed; `qpdf --check` assertions skipped");
        return (0, String::new());
    };
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

// ---- corpus helpers ----

#[derive(Clone)]
pub struct CorpusFile {
    pub path: PathBuf,
    pub password: Option<String>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            out.push(p);
        }
    }
}

/// Every PDF under `corpus/cache` (downloaded and generated), sorted. Empty when the corpus has
/// not been fetched; tests then skip.
pub fn corpus_files(max_bytes: u64) -> Vec<CorpusFile> {
    let cache = repo_root().join("corpus/cache");
    let mut paths = Vec::new();
    walk(&cache.join("files"), &mut paths);
    walk(&cache.join("generated"), &mut paths);
    paths.sort();
    let mut passwords = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(cache.join("generated/passwords.tsv")) {
        for line in t.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() >= 3 {
                let pw = if f[1].is_empty() { f[2] } else { f[1] };
                passwords.insert(f[0].to_string(), pw.to_string());
            }
        }
    }
    paths
        .into_iter()
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= max_bytes))
        .map(|p| {
            let password = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| passwords.get(n).cloned());
            CorpusFile { path: p, password }
        })
        .collect()
}

/// Evenly spaced sample of `n` items (all of them when fewer).
pub fn sample<T: Clone>(items: &[T], n: usize) -> Vec<T> {
    if items.len() <= n {
        return items.to_vec();
    }
    (0..n).map(|i| items[i * items.len() / n].clone()).collect()
}

pub fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

use papyrine_cos::{Object, ObjectKind};

pub fn random_value(doc: &Document, rng: &mut Rng, ids: &[ObjId], depth: u32) -> Object {
    let top = if depth >= 3 { 7 } else { 10 };
    match rng.below(top) {
        0 => doc.new_int((rng.next() as i64) >> rng.below(60)),
        1 => {
            let texts = [
                "0.0000001",
                "-3.25",
                "123456.789",
                ".5",
                "-.0001",
                "42.",
                "1.000000",
            ];
            doc.parse_object(texts[rng.below(texts.len())]).unwrap()
        }
        2 => {
            let n = rng.below(12);
            let b: Vec<u8> = (0..n).map(|_| (rng.next() % 255 + 1) as u8).collect();
            doc.new_name(b).unwrap()
        }
        3 => {
            let n = rng.below(40);
            doc.new_string(rng.bytes(n)).unwrap()
        }
        4 => doc
            .new_string("plain (nested (parens)) and \\ slash\r\n")
            .unwrap(),
        5 => doc.new_bool(rng.below(2) == 0),
        6 => doc.new_null(),
        7 if depth < 3 => {
            let a = doc.new_array();
            for _ in 0..rng.below(4) {
                a.array_push(&random_value(doc, rng, ids, depth + 1))
                    .unwrap();
            }
            a
        }
        8 if depth < 3 => {
            let d = doc.new_dict();
            for i in 0..rng.below(4) {
                d.dict_set(format!("K{i}"), &random_value(doc, rng, ids, depth + 1))
                    .unwrap();
            }
            d
        }
        9 if depth < 3 && !ids.is_empty() => doc.object(ids[rng.below(ids.len())]).unwrap(),
        _ => doc.new_int(7),
    }
}

/// Apply `n` random mutations; returns the ids of changed and created objects.
pub fn random_edits(doc: &Document, rng: &mut Rng, ids: &[ObjId], n: usize) -> Vec<ObjId> {
    let mut dirty = Vec::new();
    // The /Encrypt dictionary and everything it references are read before decryption is set
    // up, so a test must not hang new objects off them.
    let mut protected = std::collections::BTreeSet::new();
    if let Some(e) = doc.trailer().unwrap().dict_get("Encrypt").unwrap().id() {
        let mut todo = vec![e];
        while let Some(id) = todo.pop() {
            if !protected.insert(id) {
                continue;
            }
            let repr = doc.fingerprint(&doc.object(id).unwrap()).unwrap().repr;
            let t = String::from_utf8_lossy(&repr).into_owned();
            let toks: Vec<&str> = t.split_whitespace().collect();
            for w in toks.windows(3) {
                if w[2].trim_end_matches(['>', ']']) == "R"
                    && let (Ok(n), Ok(g)) = (w[0].parse::<u32>(), w[1].parse::<u16>())
                {
                    todo.push(ObjId::new(n, g));
                }
            }
        }
    }
    for _ in 0..n {
        let id = ids[rng.below(ids.len())];
        if protected.contains(&id) {
            continue;
        }
        let obj = doc.object(id).unwrap();
        let kind = obj.kind().unwrap();
        if kind == ObjectKind::Stream {
            let t = obj.stream_dict().unwrap().dict_get("Type").unwrap();
            if t.kind().unwrap() == ObjectKind::Name
                && matches!(t.name().unwrap().as_slice(), b"XRef" | b"ObjStm")
            {
                continue;
            }
        }
        match kind {
            ObjectKind::Dictionary | ObjectKind::Stream => {
                let v = random_value(doc, rng, ids, 0);
                obj.dict_set(format!("PapyrineTest{}", rng.below(3)), &v)
                    .unwrap();
                if kind == ObjectKind::Stream && rng.below(3) == 0 {
                    // Valid content syntax (a comment) around arbitrary bytes, so qpdf --check can
                    // still parse the stream if it is a page's content.
                    let len = rng.below(300);
                    let mut data = b"q Q\n% ".to_vec();
                    data.extend(
                        rng.bytes(len)
                            .into_iter()
                            .filter(|b| *b != b'\n' && *b != b'\r'),
                    );
                    data.push(b'\n');
                    obj.stream_replace(&data, None, None).unwrap();
                }
                if rng.below(4) == 0 {
                    // A brand new indirect object referenced from here.
                    let d = doc.parse_object("<< /Type /PapyrineNew /S (x) >>").unwrap();
                    d.dict_set("V", &random_value(doc, rng, ids, 1)).unwrap();
                    let d = doc.make_indirect(&d).unwrap();
                    obj.dict_set("PapyrineRef", &d).unwrap();
                    dirty.push(d.id().unwrap());
                }
            }
            ObjectKind::Array => {
                // Leave arrays of references (/Kids, /Annots...) alone so edits stay structurally
                // plausible for qpdf --check.
                if obj.array_items().unwrap().iter().any(|i| i.is_indirect()) {
                    continue;
                }
                obj.array_push(&random_value(doc, rng, ids, 0)).unwrap();
            }
            _ => continue,
        }
        dirty.push(id);
    }
    dirty.sort();
    dirty.dedup();
    dirty
}
