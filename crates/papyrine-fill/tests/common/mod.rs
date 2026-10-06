#![allow(dead_code)]

use std::collections::BTreeMap;
use std::process::Command as Proc;
use std::sync::Arc;

use papyrine_cos::{Document, ObjId, OpenOptions, WriteOptions};
use papyrine_ops::{Command, History, ObjectImage, TRAILER};
use papyrine_render as render;

/// Assemble a PDF from object bodies (object numbers are 1-based, in push order).
#[derive(Default)]
pub struct Pdf {
    objs: Vec<Vec<u8>>,
}

impl Pdf {
    pub fn new() -> Pdf {
        Pdf::default()
    }
    /// Reserve the next object number without a body yet.
    pub fn next_num(&self) -> usize {
        self.objs.len() + 1
    }
    pub fn add(&mut self, body: impl AsRef<[u8]>) -> usize {
        self.objs.push(body.as_ref().to_vec());
        self.objs.len()
    }
    pub fn set(&mut self, num: usize, body: impl AsRef<[u8]>) {
        self.objs[num - 1] = body.as_ref().to_vec();
    }
    pub fn stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let mut b = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        b.extend_from_slice(data);
        b.extend_from_slice(b"\nendstream");
        self.add(b)
    }
    pub fn build(&self, root: usize) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in self.objs.iter().enumerate() {
            offs.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            out.extend_from_slice(o);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        let n = self.objs.len() + 1;
        out.extend(format!("xref\n0 {n}\n0000000000 65535 f \n").bytes());
        for o in offs {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {n} /Root {root} 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .bytes(),
        );
        out
    }
}

pub fn open(data: Vec<u8>) -> Document {
    Document::open_bytes(data, &OpenOptions::default()).expect("open")
}

pub fn write(doc: &Document) -> Vec<u8> {
    doc.write(&WriteOptions {
        static_id: true,
        ..WriteOptions::default()
    })
    .expect("write")
    .into_vec()
}

pub fn roundtrip(doc: &Document) -> Document {
    open(write(doc))
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

/// Apply, then check exact undo and redo; leaves the command applied.
pub fn run(doc: &Document, h: &mut History, c: impl Command + 'static) {
    let before = snapshot(doc);
    h.execute(doc, Box::new(c)).expect("execute");
    let after = snapshot(doc);
    h.undo(doc).unwrap().expect("undo");
    assert_matches(doc, &before, "after undo");
    h.redo(doc).unwrap().expect("redo");
    assert_matches(doc, &after, "after redo");
}

/// `qpdf --check` on the bytes; panics with its output on failure (warnings are fine).
pub fn qpdf_check(bytes: &[u8], what: &str) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.pdf");
    std::fs::write(&p, bytes).unwrap();
    let out = Proc::new("qpdf")
        .arg("--check")
        .arg(&p)
        .output()
        .expect("qpdf CLI");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    // exit 0 = clean, 3 = warnings only
    assert!(
        matches!(out.status.code(), Some(0 | 3)),
        "qpdf --check failed for {what}: {text}"
    );
}

pub fn lib() -> Arc<render::Library> {
    render::Library::global().expect("libpdfium: run tools/fetch-pdfium")
}

/// RGBA of the whole page at `scale` px per point (white background).
pub fn render_page(bytes: &[u8], page: usize, max_edge: u32) -> render::Tile {
    let mut d = render::Document::open(&lib(), render::bytes_from_vec(bytes.to_vec()), &[], None)
        .expect("pdfium open");
    d.render_preview(page, max_edge, &mut render::never_cancel)
        .expect("render")
}

/// Count of non-white pixels inside a PDF-space rectangle of a page rendered with
/// `render_page` (`page_h` = page height in points).
pub fn ink_in(t: &render::Tile, scale: f64, page_h: f64, r: [f64; 4]) -> usize {
    let (x0, x1) = ((r[0] * scale) as u32, (r[2] * scale) as u32);
    let (y0, y1) = (
        ((page_h - r[3]) * scale) as u32,
        ((page_h - r[1]) * scale) as u32,
    );
    let mut n = 0;
    for y in y0..y1.min(t.height) {
        for x in x0..x1.min(t.width) {
            let p = t.pixel(x, y);
            if p[0] < 200 || p[1] < 200 || p[2] < 200 {
                n += 1;
            }
        }
    }
    n
}

/// Non-white pixels inside a device-space rectangle `[x0 y0 x1 y1]`.
pub fn ink_dev(t: &render::Tile, r: [f64; 4]) -> usize {
    let mut n = 0;
    for y in (r[1].max(0.0) as u32)..(r[3] as u32).min(t.height) {
        for x in (r[0].max(0.0) as u32)..(r[2] as u32).min(t.width) {
            let p = t.pixel(x, y);
            if p[0] < 200 || p[1] < 200 || p[2] < 200 {
                n += 1;
            }
        }
    }
    n
}

pub struct FormFile {
    pub bytes: Vec<u8>,
}

/// A one-page form with every field type. Field rects are listed in `rects`.
pub fn sample_form() -> Vec<u8> {
    let mut p = Pdf::new();
    // 1 catalog, 2 pages, 3 page, 4 font Helv, 5 font ZaDb
    p.add("<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>");
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add("PLACEHOLDER-PAGE");
    p.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    p.add("<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>");
    p.add("PLACEHOLDER-ACRO");
    let mut kids = Vec::new();
    fn addf(p: &mut Pdf, kids: &mut Vec<usize>, body: String) -> usize {
        let n = p.add(body);
        kids.push(n);
        n
    }
    let base = "/Type /Annot /Subtype /Widget /F 4 /DA (/Helv 12 Tf 0 g) /MK << /BC [0] /BG [0.95] >> /BS << /W 1 /S /S >>";
    let tx = format!("{base} /FT /Tx");
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (name) /Rect [50 700 250 720] /P 3 0 R >>"),
    );
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (comb) /Ff 16777216 /MaxLen 8 /Rect [50 660 250 680] /P 3 0 R >>"),
    );
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (notes) /Ff 4096 /Rect [50 560 250 640] /P 3 0 R >>"),
    );
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (pwd) /Ff 8192 /Rect [50 520 250 540] /P 3 0 R >>"),
    );
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (centered) /Q 1 /Rect [300 700 500 720] /P 3 0 R >>"),
    );
    addf(
        &mut p,
        &mut kids,
        format!("<< {tx} /T (right) /Q 2 /Rect [300 660 500 680] /P 3 0 R >>"),
    );
    // Checkbox with appearances.
    let on = p.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 14 14]",
        b"0 g 2 2 10 10 re f",
    );
    let off = p.stream("/Type /XObject /Subtype /Form /BBox [0 0 14 14]", b"");
    addf(
        &mut p,
        &mut kids,
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /F 4 /V /Yes /AS /Yes /Rect [300 600 314 614] /P 3 0 R /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >> /MK << /BC [0] /CA (4) >> >>"
        ),
    );
    // Checkbox with no appearances at all.
    addf(&mut p, &mut kids,
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (bare) /F 4 /Rect [330 600 344 614] /P 3 0 R /MK << /BC [0] /BG [1] /CA (8) >> /DA (/ZaDb 0 Tf 0 g) >>".to_string(),
    );
    // Radio group of three.
    let r1 = p.add("<< /Type /Annot /Subtype /Widget /Parent RADIO /F 4 /Rect [300 560 314 574] /P 3 0 R /AS /Off /MK << /BC [0] /CA (l) >> /AP << /N << /x 0 0 R /Off 0 0 R >> >> >>");
    let r2 = p.add("<< /Type /Annot /Subtype /Widget /Parent RADIO /F 4 /Rect [330 560 344 574] /P 3 0 R /AS /Off /MK << /BC [0] /CA (l) >> >>");
    let r3 = p.add("<< /Type /Annot /Subtype /Widget /Parent RADIO /F 4 /Rect [360 560 374 574] /P 3 0 R /AS /Off /MK << /BC [0] /CA (l) >> >>");
    let radio = p.add(format!(
        "<< /FT /Btn /Ff 32768 /T (pick) /Kids [{r1} 0 R {r2} 0 R {r3} 0 R] /Opt [(x) (y) (z)] >>"
    ));
    for r in [r1, r2, r3] {
        let b = String::from_utf8(p.objs[r - 1].clone())
            .unwrap()
            .replace("RADIO", &format!("{radio} 0 R"));
        p.set(r, b);
    }
    // Fix r1's dummy AP: use real streams.
    let ron = p.stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 14 14]",
        b"0 g 4 4 6 6 re f",
    );
    let roff = p.stream("/Type /XObject /Subtype /Form /BBox [0 0 14 14]", b"");
    let b = String::from_utf8(p.objs[r1 - 1].clone()).unwrap().replace(
        "/x 0 0 R /Off 0 0 R",
        &format!("/x {ron} 0 R /Off {roff} 0 R"),
    );
    p.set(r1, b);
    kids.push(radio);
    // Combo and list.
    addf(
        &mut p,
        &mut kids,
        format!(
            "<< {base} /FT /Ch /T (country) /Ff 131072 /Opt [(France) (Germany) [(it) (Italy)]] /Rect [300 520 500 540] /P 3 0 R >>"
        ),
    );
    addf(
        &mut p,
        &mut kids,
        format!(
            "<< {base} /FT /Ch /T (langs) /Ff 2097152 /Opt [(Ada) (Basic) (C) (D) (Erlang) (Forth) (Go)] /Rect [300 420 420 500] /P 3 0 R >>"
        ),
    );
    addf(
        &mut p,
        &mut kids,
        format!(
            "<< {base} /FT /Ch /T (editable) /Ff 393216 /Opt [(One) (Two)] /Rect [300 380 500 400] /P 3 0 R >>"
        ),
    );
    // Number fields with AF scripts and a sum.
    let aa = |k: &str, f: &str| format!("/{k} << /S /JavaScript /JS ({f}) >>");
    let q = addf(
        &mut p,
        &mut kids,
        format!(
            "<< {tx} /T (qty) /Q 2 /Rect [50 480 150 500] /P 3 0 R /AA << {} {} >> >>",
            aa("K", "AFNumber_Keystroke(2, 0, 0, 0, \\\"\\\", true);"),
            aa("F", "AFNumber_Format(2, 0, 0, 0, \\\"\\\", true);")
        ),
    );
    let pr = addf(
        &mut p,
        &mut kids,
        format!(
            "<< {tx} /T (price) /Q 2 /Rect [50 450 150 470] /P 3 0 R /AA << {} {} {} >> >>",
            aa("K", "AFNumber_Keystroke(2, 0, 0, 0, \\\"$\\\", true);"),
            aa("F", "AFNumber_Format(2, 0, 1, 0, \\\"$\\\", true);"),
            aa("V", "AFRange_Validate(true, 0, true, 1000);")
        ),
    );
    let total = addf(
        &mut p,
        &mut kids,
        format!(
            "<< {tx} /T (total) /Q 2 /Rect [50 420 150 440] /P 3 0 R /AA << {} {} >> >>",
            aa(
                "C",
                "AFSimple_Calculate(\\\"SUM\\\", new Array(\\\"qty\\\", \\\"price\\\"));"
            ),
            aa("F", "AFNumber_Format(2, 0, 1, 0, \\\"\\\", true);")
        ),
    );
    addf(
        &mut p,
        &mut kids,
        format!(
            "<< {tx} /T (date) /Rect [50 390 150 410] /P 3 0 R /AA << {} {} >> >>",
            aa("K", "AFDate_KeystrokeEx(\\\"mm/dd/yyyy\\\");"),
            aa("F", "AFDate_FormatEx(\\\"d mmm yyyy\\\");")
        ),
    );
    let _ = (q, pr);
    let annots: Vec<String> = kids
        .iter()
        .flat_map(|&k| {
            let b = String::from_utf8_lossy(&p.objs[k - 1]).into_owned();
            if b.contains("/Kids") {
                vec![r1, r2, r3]
            } else {
                vec![k]
            }
        })
        .map(|k| format!("{k} 0 R"))
        .collect();
    p.set(
        3,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv 4 0 R >> >> /Annots [{}] >>",
            annots.join(" ")
        ),
    );
    let fields: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    p.set(
        6,
        format!(
            "<< /Fields [{}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R /ZaDb 5 0 R >> >> /CO [{total} 0 R] >>",
            fields.join(" ")
        ),
    );
    p.build(1)
}

// ----- test-only oracles: Poppler, pdf.js, tesseract ---------------------------------------

fn scratch(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.pdf");
    std::fs::write(&p, bytes).unwrap();
    (dir, p)
}

pub fn tool_available(name: &str) -> bool {
    Proc::new(name).arg("-v").output().is_ok() || Proc::new(name).arg("--version").output().is_ok()
}

/// Poppler's text extraction (includes widget appearance text).
pub fn pdftotext(bytes: &[u8]) -> String {
    let (_d, p) = scratch(bytes);
    let out = Proc::new("pdftotext")
        .arg("-layout")
        .arg(&p)
        .arg("-")
        .output()
        .expect("pdftotext");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Where `npm install pdfjs-dist@4` was run: `PDFJS_ORACLE_DIR`, else `<workspace>/target/fill-oracle`
/// (also found next to a custom `CARGO_TARGET_DIR`).
pub fn pdfjs_dir() -> std::path::PathBuf {
    let marker = "node_modules/pdfjs-dist/legacy/build/pdf.mjs";
    let mut cands: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("PDFJS_ORACLE_DIR") {
        cands.push(d.into());
    }
    cands.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fill-oracle"));
    if let Some(t) = std::env::var_os("CARGO_TARGET_DIR") {
        cands.push(std::path::Path::new(&t).join("../fill-oracle"));
    }
    cands
        .iter()
        .find(|c| c.join(marker).exists())
        .and_then(|c| c.canonicalize().ok())
        .unwrap_or_else(|| cands[0].clone())
}

pub fn pdfjs_available() -> bool {
    pdfjs_dir()
        .join("node_modules/pdfjs-dist/legacy/build/pdf.mjs")
        .exists()
        && tool_available("node")
}

/// What pdf.js draws for each widget (text of its appearance) and reads as the field value.
pub fn pdfjs_widgets(bytes: &[u8]) -> Vec<serde_json::Value> {
    let (_d, p) = scratch(bytes);
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/pdfjs_widgets.mjs");
    let out = Proc::new("node")
        .arg(script)
        .arg(pdfjs_dir())
        .arg(&p)
        .output()
        .expect("node");
    assert!(
        out.status.success(),
        "pdf.js oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("pdf.js json")
}

pub fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("")
}

/// OCR of a rendered tile (tesseract reads the PPM we write).
pub fn ocr(t: &render::Tile) -> String {
    ocr_psm(t, 6)
}

fn ocr_psm(t: &render::Tile, psm: u32) -> String {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("p.ppm");
    let mut data = format!("P6\n{} {}\n255\n", t.width, t.height).into_bytes();
    for px in t.rgba.as_chunks::<4>().0 {
        data.extend_from_slice(&px[..3]);
    }
    std::fs::write(&p, data).unwrap();
    let out = Proc::new("tesseract")
        .arg(&p)
        .arg("-")
        .arg("--psm")
        .arg(psm.to_string())
        .output()
        .expect("tesseract");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A sub-image of a page render, padded with white so tesseract has margins.
pub fn crop_tile(t: &render::Tile, scale: f64, page_h: f64, r: [f64; 4]) -> render::Tile {
    let (x0, x1) = ((r[0] * scale) as u32, (r[2] * scale) as u32);
    let (y0, y1) = (
        ((page_h - r[3]) * scale) as u32,
        ((page_h - r[1]) * scale) as u32,
    );
    let pad = 12u32;
    let (w, h) = (x1 - x0 + 2 * pad, y1 - y0 + 2 * pad);
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    for y in y0..y1.min(t.height) {
        for x in x0..x1.min(t.width) {
            let p = t.pixel(x, y);
            let i = (((y - y0 + pad) * w + (x - x0 + pad)) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&p);
        }
    }
    render::Tile {
        width: w,
        height: h,
        rgba,
    }
}

/// OCR of a single line of text.
pub fn ocr_line(t: &render::Tile) -> String {
    ocr_psm(t, 7)
}

/// Poppler's raster of page 0 at `dpi` as an `render::Tile`-like RGB buffer (via PPM).
pub fn poppler_ppm(bytes: &[u8], dpi: u32) -> (u32, u32, Vec<u8>) {
    let (d, p) = scratch(bytes);
    let prefix = d.path().join("pop");
    let st = Proc::new("pdftoppm")
        .args(["-r", &dpi.to_string(), "-f", "1", "-l", "1", "-singlefile"])
        .arg(&p)
        .arg(&prefix)
        .status()
        .expect("pdftoppm");
    assert!(st.success());
    let ppm = std::fs::read(prefix.with_extension("ppm")).expect("ppm");
    // P6\nW H\n255\n
    let mut it = ppm.splitn(4, |&b| b == b'\n');
    it.next();
    let dims = String::from_utf8_lossy(it.next().unwrap()).into_owned();
    it.next();
    let data = it.next().unwrap().to_vec();
    let mut wh = dims.split_whitespace().map(|v| v.parse::<u32>().unwrap());
    (wh.next().unwrap(), wh.next().unwrap(), data)
}
