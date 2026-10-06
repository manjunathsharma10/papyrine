#![allow(dead_code)]

use std::path::{Path, PathBuf};

use papyrine_core::CancelToken;
use papyrine_cos::{Document, OpenOptions};
use papyrine_engine::proto::*;
use papyrine_engine::state::document_digest;
use papyrine_engine::{EngineHandler, default_registry};
use papyrine_ipc::{CommandRequest, DocId, DocSource, IpcError};
use serde_json::{Value, json};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A corpus file by name: generated files first, then the fetched ones (`js-forms/...`).
pub fn corpus(name: &str) -> Option<Vec<u8>> {
    let root = repo_root().join("corpus/cache");
    std::fs::read(root.join("generated").join(name))
        .or_else(|_| std::fs::read(root.join(name)))
        .ok()
}

/// A valid classic-xref PDF with `pages` pages.
pub fn build_pdf(pages: usize) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: String| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    obj(
        &mut out,
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] >>",
            kids.join(" ")
        ),
    );
    obj(
        &mut out,
        "<< /Producer (engine-tests) /Title (Test document) >>".into(),
    );
    for i in 0..pages {
        let content = format!("BT /F1 12 Tf 72 720 Td (Page {}) Tj ET\n", i + 1);
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>",
                5 + 2 * i
            ),
        );
        obj(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                content
            ),
        );
    }
    let xref_pos = out.len();
    let n = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{xref_pos}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

pub struct Eng {
    pub h: EngineHandler,
    pub cancel: CancelToken,
    pub dir: tempfile::TempDir,
}

impl Eng {
    pub fn new() -> Eng {
        Eng::with_registry(default_registry())
    }

    pub fn with_registry(r: papyrine_ops::CommandRegistry) -> Eng {
        let dir = tempfile::tempdir().unwrap();
        Eng {
            h: EngineHandler::with_scratch(r, dir.path().to_path_buf()),
            cancel: CancelToken::new(),
            dir,
        }
    }

    pub fn try_call(&mut self, req: Request) -> Result<Response, IpcError> {
        self.h.dispatch(req, &self.cancel)
    }

    pub fn call(&mut self, req: Request) -> Response {
        self.try_call(req)
            .unwrap_or_else(|e| panic!("request failed: {e}"))
    }

    pub fn open(&mut self, doc: u64, bytes: Vec<u8>) -> DocSummary {
        self.try_open(doc, bytes, None).unwrap()
    }

    pub fn try_open(
        &mut self,
        doc: u64,
        bytes: Vec<u8>,
        password: Option<&str>,
    ) -> Result<DocSummary, IpcError> {
        match self.try_call(Request::Open {
            doc: DocId(doc),
            source: DocSource::Bytes(bytes),
            password: password.map(str::to_owned),
            params: OpenParams::default(),
        })? {
            Response::Opened(s) => Ok(*s),
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn try_exec(&mut self, doc: u64, name: &str, params: Value) -> Result<Committed, IpcError> {
        match self.try_call(Request::Execute {
            doc: DocId(doc),
            command: CommandRequest {
                name: name.into(),
                params_json: params.to_string(),
            },
            blobs: vec![],
            out_section: None,
        })? {
            Response::Committed(c) => Ok(*c),
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn exec(&mut self, doc: u64, name: &str, params: Value) -> Committed {
        self.try_exec(doc, name, params)
            .unwrap_or_else(|e| panic!("{name} failed: {e}"))
    }

    pub fn undo(&mut self, doc: u64) -> Committed {
        match self.call(Request::Undo {
            doc: DocId(doc),
            out_section: None,
        }) {
            Response::Committed(c) => *c,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn redo(&mut self, doc: u64) -> Committed {
        match self.call(Request::Redo {
            doc: DocId(doc),
            out_section: None,
        }) {
            Response::Committed(c) => *c,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn digest(&mut self, doc: u64) -> DigestInfo {
        match self.call(Request::Digest {
            doc: DocId(doc),
            per_object: true,
        }) {
            Response::Digest(d) => d,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn query(&mut self, doc: u64, q: Query) -> QueryResult {
        match self.call(Request::Query {
            doc: DocId(doc),
            query: q,
        }) {
            Response::Query(r) => *r,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn save(&mut self, doc: u64, mode: SaveMode) -> Response {
        self.call(Request::Save {
            doc: DocId(doc),
            mode,
        })
    }
}

pub fn read_delivery(d: &Delivery) -> Vec<u8> {
    match d {
        Delivery::Inline(v) => v.clone(),
        Delivery::File(f) => std::fs::read(&f.path).unwrap(),
        Delivery::Shared { .. } => panic!("no region was passed"),
    }
}

/// What the renderer would hold: base plus sections, maintained from `SnapshotUpdate`s.
pub struct Mirror {
    pub base: Vec<u8>,
    pub sections: Vec<Vec<u8>>,
    pub epoch: u64,
    pub password: Option<String>,
}

impl Mirror {
    pub fn new(original: Vec<u8>) -> Mirror {
        Mirror {
            base: original,
            sections: vec![],
            epoch: 0,
            password: None,
        }
    }

    pub fn from_open(original: Vec<u8>, s: &DocSummary) -> Mirror {
        match &s.render_base {
            RenderBase::Original => Mirror::new(original),
            RenderBase::Repaired(f) => Mirror::new(std::fs::read(&f.path).unwrap()),
        }
    }

    pub fn apply(&mut self, u: &SnapshotUpdate, original_for_none: Option<&[u8]>) {
        assert!(u.epoch >= self.epoch, "epoch went backwards");
        match &u.action {
            SnapshotAction::Unchanged => {}
            SnapshotAction::Append(d) => self.sections.push(read_delivery(d)),
            SnapshotAction::ReplaceSections(d) => self.sections = vec![read_delivery(d)],
            SnapshotAction::NewBase(Some(f)) => {
                self.base = std::fs::read(&f.path).unwrap();
                self.sections.clear();
            }
            SnapshotAction::NewBase(None) => {
                self.base = original_for_none.expect("saved file").to_vec();
                self.sections.clear();
            }
        }
        assert_eq!(
            self.sections.len() as u32,
            u.sections,
            "section count mismatch"
        );
        self.epoch = u.epoch;
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut v = self.base.clone();
        for s in &self.sections {
            v.extend_from_slice(s);
        }
        v
    }

    pub fn doc(&self) -> Document {
        let opts = match &self.password {
            Some(p) => OpenOptions::with_password(p.as_str()),
            None => OpenOptions::default(),
        };
        Document::open_bytes(self.bytes(), &opts).unwrap()
    }
}

/// Every live object must appear in the mirrored snapshot, byte for byte.
pub fn assert_snapshot_matches(e: &mut Eng, doc: u64, m: &Mirror) {
    let live = e.digest(doc);
    let sd = m.doc();
    assert!(
        sd.repair_log().is_empty(),
        "snapshot needed repairs: {:?}",
        sd.repair_log()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    let snap = document_digest(&sd, true).unwrap();
    let snap: std::collections::HashMap<(u32, u16), [u8; 32]> = snap
        .per_object
        .into_iter()
        .map(|(n, g, h)| ((n, g), h))
        .collect();
    for (n, g, h) in &live.per_object {
        if *n == 0 {
            continue;
        }
        match snap.get(&(*n, *g)) {
            Some(sh) => assert_eq!(sh, h, "object {n} {g} differs in the snapshot"),
            None => panic!("object {n} {g} missing from the snapshot"),
        }
    }
    assert_eq!(
        sd.page_count().unwrap() as u32,
        match e.query(doc, Query::Summary) {
            QueryResult::Summary(s) => s.page_count,
            _ => unreachable!(),
        }
    );
}

pub fn rotate(pages: &[usize], delta: i32) -> (&'static str, Value) {
    ("rotate_pages", json!({"pages": pages, "delta": delta}))
}

pub fn have_qpdf() -> bool {
    std::process::Command::new("qpdf")
        .arg("--version")
        .output()
        .is_ok()
}

/// `qpdf --check` on `bytes`; returns stdout+stderr when it fails.
pub fn qpdf_check(bytes: &[u8], dir: &Path) -> Result<(), String> {
    let p = dir.join(format!("check-{}.pdf", bytes.len()));
    std::fs::write(&p, bytes).unwrap();
    let out = std::process::Command::new("qpdf")
        .arg("--check")
        .arg(&p)
        .output()
        .unwrap();
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr))
    }
}
