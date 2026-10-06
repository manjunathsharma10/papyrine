#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use papyrine_cos::{Document, ObjId, OpenOptions, WriteOptions};
use papyrine_ops::{Command as OpsCommand, History, ObjectImage, TRAILER};

pub const PAGE_W: f64 = 400.0;
pub const PAGE_H: f64 = 400.0;

/// A classic-xref PDF with `pages` 400x400 pages; page 1 says "Hello World" in Helvetica.
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
    obj(&mut out, "<< /Producer (papyrine-tests) >>".into());
    for i in 0..pages {
        let content = format!("BT /F1 24 Tf 20 360 Td (Hello World {}) Tj ET\n", i + 1);
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Contents {} 0 R \
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

pub fn open(data: Vec<u8>) -> Document {
    Document::open_bytes(data, &OpenOptions::default()).expect("open")
}

pub fn write_bytes(doc: &Document) -> Vec<u8> {
    doc.write(&WriteOptions {
        static_id: true,
        ..WriteOptions::default()
    })
    .expect("write")
    .into_vec()
}

pub fn roundtrip(doc: &Document) -> Document {
    open(write_bytes(doc))
}

pub type Snapshot = BTreeMap<ObjId, ObjectImage>;

pub fn snapshot(doc: &Document) -> Snapshot {
    doc.object_ids()
        .unwrap()
        .into_iter()
        .chain([TRAILER])
        .map(|id| (id, ObjectImage::capture(doc, id).unwrap()))
        .collect()
}

pub fn assert_matches(doc: &Document, snap: &Snapshot, what: &str) {
    for (id, img) in snap {
        let now = ObjectImage::capture(doc, *id).unwrap();
        assert_eq!(&now, img, "{what}: object {id} differs");
    }
}

pub fn history() -> History {
    let mut h = History::new();
    h.set_verify(true);
    h
}

pub fn registry() -> papyrine_ops::CommandRegistry {
    let mut r = papyrine_ops::CommandRegistry::with_builtin();
    papyrine_annotate::register(&mut r);
    r
}

/// Execute `cmd` with the shadow verifier on and return the id of the created annotation.
pub fn run(
    doc: &Document,
    h: &mut History,
    cmd: impl OpsCommand + 'static,
) -> papyrine_ops::ChangeSummary {
    h.execute(doc, Box::new(cmd)).expect("execute")
}

pub fn tools_required() -> bool {
    std::env::var_os("PAPYRINE_REQUIRE_ORACLES").is_some()
}

pub fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false)
}

pub fn scratch(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(name);
    std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
    (d, p)
}

/// `qpdf --check`; returns Err(output) on any warning or error.
pub fn qpdf_check(bytes: &[u8]) -> Result<(), String> {
    if !have("qpdf") {
        assert!(!tools_required(), "qpdf missing");
        eprintln!("qpdf not installed: skipping --check");
        return Ok(());
    }
    let (_d, p) = scratch("check.pdf", bytes);
    let o = Command::new("qpdf")
        .arg("--check")
        .arg(&p)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    if o.status.success() && !text.contains("WARNING") && !text.contains("ERROR") {
        Ok(())
    } else {
        Err(text)
    }
}

/// An RGBA page raster at 1 pt per pixel.
pub struct Raster {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    /// Pixel at page coordinates (x, y) with the PDF origin at the bottom-left.
    pub fn at(&self, x: f64, y: f64) -> [u8; 3] {
        let px = (x.floor() as i64).clamp(0, i64::from(self.w) - 1) as u32;
        let py = ((f64::from(self.h) - y).floor() as i64).clamp(0, i64::from(self.h) - 1) as u32;
        let i = ((py * self.w + px) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }
}

pub fn pdfium_render(bytes: Vec<u8>, page: usize) -> Raster {
    let lib = papyrine_render::Library::global().expect("libpdfium: run tools/fetch-pdfium");
    let mut d =
        papyrine_render::Document::open(&lib, papyrine_render::bytes_from_vec(bytes), &[], None)
            .expect("pdfium open");
    let (w, h) = d.page_size(page).unwrap();
    let t = d
        .render_preview(page, w.max(h) as u32, &mut papyrine_render::never_cancel)
        .unwrap();
    Raster {
        w: t.width,
        h: t.height,
        rgba: t.rgba,
    }
}

pub fn is_near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(&b)
        .all(|(x, y)| (i32::from(*x) - i32::from(*y)).abs() <= tol)
}

pub const WHITE: [u8; 3] = [255, 255, 255];

pub fn model_annots(doc: Document, page: usize) -> Vec<papyrine_model::Annotation> {
    let m = papyrine_model::Model::new(doc);
    let set = m.annotations(page).unwrap();
    set.items.clone()
}

pub fn pdftoppm(bytes: &[u8], page: usize) -> Option<Raster> {
    if !have("pdftoppm") {
        assert!(!tools_required(), "pdftoppm missing");
        eprintln!("pdftoppm not installed: skipping");
        return None;
    }
    let (d, p) = scratch("in.pdf", bytes);
    let out = d.path().join("out");
    let st = Command::new("pdftoppm")
        .args([
            "-r",
            "72",
            "-f",
            &(page + 1).to_string(),
            "-l",
            &(page + 1).to_string(),
            "-singlefile",
        ])
        .arg(&p)
        .arg(&out)
        .status()
        .ok()?;
    assert!(st.success(), "pdftoppm failed");
    let ppm = std::fs::read(d.path().join("out.ppm")).ok()?;
    Some(parse_ppm(&ppm))
}

fn parse_ppm(b: &[u8]) -> Raster {
    // "P6\nW H\n255\n" then RGB bytes.
    let mut pos = 0;
    let mut tok = || {
        while b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let s = pos;
        while !b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        String::from_utf8_lossy(&b[s..pos]).into_owned()
    };
    assert_eq!(tok(), "P6");
    let w: u32 = tok().parse().unwrap();
    let h: u32 = tok().parse().unwrap();
    let _max = tok();
    pos += 1;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in b[pos..].as_chunks::<3>().0.iter().take((w * h) as usize) {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Raster { w, h, rgba }
}

pub fn _arc_unused(_: Arc<()>, _: &Path) {}

/// The annotation objects listed in a page's /Annots.
pub fn page_annots(doc: &Document, page: usize) -> Vec<papyrine_cos::Object> {
    let a = doc.page(page).unwrap().dict_get("Annots").unwrap();
    a.array_items().unwrap_or_default()
}
