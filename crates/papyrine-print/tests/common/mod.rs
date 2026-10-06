//! Shared helpers for the print tests: tiny PDF builders (no xref; qpdf reconstructs it),
//! PDFium rendering, pixel probes and `qpdf --check`.
#![allow(dead_code)]

use papyrine_core::Rect;
use papyrine_render::{Document, Library, Tile, bytes_from_vec};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< /Length {} {dict} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// Assemble objects into a PDF without an xref table (recovery rebuilds it).
pub fn assemble(objs: &[(u32, String)], root: u32) -> Vec<u8> {
    let mut s = String::from("%PDF-1.7\n");
    for (id, body) in objs {
        s.push_str(&format!("{id} 0 obj\n{body}\nendobj\n"));
    }
    s.push_str(&format!("trailer\n<< /Root {root} 0 R >>\n%%EOF\n"));
    s.into_bytes()
}

pub struct PageDef {
    pub w: f64,
    pub h: f64,
    pub rotate: i32,
    pub content: String,
}

pub fn black_rect(x: f64, y: f64, w: f64, h: f64) -> String {
    format!("0 0 0 rg {x} {y} {w} {h} re f")
}

/// One content-only page per entry.
pub fn pages(defs: &[PageDef]) -> Vec<u8> {
    let n = defs.len() as u32;
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 10 + 2 * i)).collect();
    let mut objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
        (2, format!("<< /Type /Pages /Count {n} /Kids [{kids}] >>")),
        (
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ),
    ];
    for (i, d) in defs.iter().enumerate() {
        let (pid, cid) = (10 + 2 * i as u32, 11 + 2 * i as u32);
        objs.push((
            pid,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Rotate {} /Contents {cid} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
                d.w, d.h, d.rotate
            ),
        ));
        objs.push((cid, stream("", &d.content)));
    }
    assemble(&objs, 1)
}

/// `n` pages whose width encodes the page number (300 + 10 i), to prove which pages were kept.
pub fn numbered(n: usize) -> Vec<u8> {
    let defs: Vec<PageDef> = (0..n)
        .map(|i| PageDef {
            w: 300.0 + 10.0 * i as f64,
            h: 400.0,
            rotate: 0,
            content: black_rect(10.0, 10.0, 50.0, 50.0),
        })
        .collect();
    pages(&defs)
}

pub const RED: [u8; 3] = [255, 0, 0];

/// Two pages. Page 2 carries printable/non-printable/hidden squares, widgets and a link.
pub fn annotated() -> Vec<u8> {
    let sq = |rect: &str, f: u32, ap: u32| {
        format!(
            "<< /Type /Annot /Subtype /Square /Rect [{rect}] /F {f} /C [0 0 0] /AP << /N {ap} 0 R >> >>"
        )
    };
    let ap = |rgb: &str| {
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 100]",
            &format!("{rgb} rg 0 0 100 100 re f"),
        )
    };
    let widget = |name: &str, val: &str, rect: &str, f: u32| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /V ({val}) /Rect [{rect}] /F {f} /DA (/Helv 14 Tf 0 g) >>"
        )
    };
    let objs = vec![
        (
            1,
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [13 0 R 14 0 R] /NeedAppearances true /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        ),
        (2, "<< /Type /Pages /Count 2 /Kids [4 0 R 5 0 R] >>".to_string()),
        (
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ),
        (
            4,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << >> >>".to_string(),
        ),
        (
            5,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R] >>".to_string(),
        ),
        (6, stream("", &black_rect(400.0, 600.0, 100.0, 100.0))),
        (10, sq("50 50 150 150", 4, 20)),
        (11, sq("200 50 300 150", 0, 21)),
        (12, sq("350 50 450 150", 6, 22)),
        (13, widget("name", "Hello", "50 200 250 240", 4)),
        (14, widget("other", "Nope", "300 200 500 240", 0)),
        (
            15,
            "<< /Type /Annot /Subtype /Link /Rect [10 10 40 40] /F 4 /Border [0 0 1] >>".to_string(),
        ),
        (20, ap("1 0 0")),
        (21, ap("0 0 1")),
        (22, ap("0 1 0")),
    ];
    assemble(&objs, 1)
}

pub fn lib() -> std::sync::Arc<Library> {
    Library::global().expect("libpdfium: run tools/fetch-pdfium")
}

pub fn open(bytes: &[u8]) -> Document {
    Document::open(&lib(), bytes_from_vec(bytes.to_vec()), &[], None).expect("pdfium open")
}

pub fn page_sizes(bytes: &[u8]) -> Vec<(f32, f32)> {
    open(bytes).page_sizes().unwrap()
}

/// Render page `i` at about one pixel per point.
pub fn render(bytes: &[u8], i: usize) -> Tile {
    let mut d = open(bytes);
    let (w, h) = d.page_size(i).unwrap();
    d.render_preview(i, w.max(h).round() as u32, &mut || false)
        .unwrap()
}

/// Pixel at PDF user-space point (x, y) of an unrotated 1 px/pt render of height `h` points.
pub fn at(t: &Tile, x: f64, y: f64) -> [u8; 3] {
    let dy = (t.height as f64 - y)
        .floor()
        .clamp(0.0, t.height as f64 - 1.0) as u32;
    let dx = x.floor().clamp(0.0, t.width as f64 - 1.0) as u32;
    let p = t.pixel(dx, dy);
    [p[0], p[1], p[2]]
}

/// Count of dark (text/ink) pixels inside a PDF-space rectangle.
pub fn dark_in(t: &Tile, r: Rect) -> usize {
    let mut n = 0;
    for dy in 0..t.height {
        for dx in 0..t.width {
            let (x, y) = (dx as f64 + 0.5, t.height as f64 - (dy as f64 + 0.5));
            if x >= r.x0 && x <= r.x1 && y >= r.y0 && y <= r.y1 {
                let p = t.pixel(dx, dy);
                if p[0] < 110 && p[1] < 110 && p[2] < 110 {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Bounding box of non-white pixels in PDF space, or None for a blank page.
pub fn ink_bbox(t: &Tile) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    let mut any = false;
    for dy in 0..t.height {
        for dx in 0..t.width {
            let p = t.pixel(dx, dy);
            if p[0] < 200 || p[1] < 200 || p[2] < 200 {
                any = true;
                x0 = x0.min(dx);
                x1 = x1.max(dx + 1);
                y0 = y0.min(dy);
                y1 = y1.max(dy + 1);
            }
        }
    }
    any.then(|| {
        Rect::new(
            x0 as f64,
            t.height as f64 - y1 as f64,
            x1 as f64,
            t.height as f64 - y0 as f64,
        )
    })
}

pub fn assert_rect_close(got: Rect, want: Rect, tol: f64, what: &str) {
    let ok = (got.x0 - want.x0).abs() <= tol
        && (got.y0 - want.y0).abs() <= tol
        && (got.x1 - want.x1).abs() <= tol
        && (got.y1 - want.y1).abs() <= tol;
    assert!(ok, "{what}: got {got:?}, want {want:?} (tol {tol})");
}

/// `qpdf --check` on the bytes (the CLI is a test tool only). Returns false when qpdf is
/// not installed so callers can skip with a note.
pub fn qpdf_check(bytes: &[u8]) -> bool {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("papyrine-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "{}.pdf",
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::File::create(&path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
    let out = match Command::new("qpdf")
        .arg("--check")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            eprintln!("qpdf CLI not found; skipping --check");
            return false;
        }
    };
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && !text.contains("WARNING"),
        "qpdf --check failed:\n{text}"
    );
    let _ = std::fs::remove_file(&path);
    true
}
