//! Fixtures and inspection helpers for the organise tests: synthetic PDFs with outlines, form
//! fields, links, named destinations and page labels, plus a summary of what a document says.
#![allow(dead_code)]

use std::collections::BTreeMap;

use papyrine_cos::{Document, ObjectKind, OpenOptions, WriteOptions};

/// What a generated document contains.
#[derive(Clone, Debug)]
pub struct Spec {
    pub tag: String,
    pub pages: usize,
    pub outline: bool,
    pub fields: bool,
    pub links: bool,
    pub dests: bool,
    pub labels: bool,
    /// Fields hang under one parent `form1` (qualified names `form1.fldN`).
    pub hier: bool,
}

impl Spec {
    pub fn plain(tag: &str, pages: usize) -> Spec {
        Spec {
            tag: tag.into(),
            pages,
            outline: false,
            fields: false,
            links: false,
            dests: false,
            labels: false,
            hier: false,
        }
    }

    pub fn rich(tag: &str, pages: usize) -> Spec {
        Spec {
            outline: true,
            fields: true,
            links: true,
            dests: true,
            labels: true,
            ..Spec::plain(tag, pages)
        }
    }
}

pub fn page_text(tag: &str, i: usize) -> String {
    format!("T:{tag} P:{i}")
}

pub fn field_name(i: usize) -> String {
    format!("fld{i}")
}

pub fn field_value(tag: &str, i: usize) -> String {
    format!("{tag}-v{i}")
}

pub fn outline_title(tag: &str, kind: &str, i: usize) -> String {
    format!("{tag} {kind} {i}")
}

struct B {
    bodies: Vec<String>,
}

impl B {
    fn set(&mut self, num: usize, body: String) {
        if self.bodies.len() < num {
            self.bodies.resize(num, String::new());
        }
        self.bodies[num - 1] = body;
    }
}

fn stream(content: &str, dict: &str) -> String {
    format!(
        "<< /Length {} {dict} >>\nstream\n{content}\nendstream",
        content.len() + 1
    )
}

const FIRST_PAGE_OBJ: usize = 10;
const PER_PAGE: usize = 5;

fn page_obj(i: usize) -> usize {
    FIRST_PAGE_OBJ + PER_PAGE * (i - 1)
}

/// Build the document described by `s` (1-based page numbers in titles and names).
pub fn build(s: &Spec) -> Vec<u8> {
    let n = s.pages;
    let mut b = B { bodies: vec![] };
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    b.set(4, font.into());
    let outlines_root = FIRST_PAGE_OBJ + PER_PAGE * n;
    let mut catalog = String::from("<< /Type /Catalog /Pages 2 0 R");
    if s.outline {
        catalog += &format!(" /Outlines {outlines_root} 0 R /PageMode /UseOutlines");
    }
    if s.fields {
        let fields: Vec<String> = if s.hier {
            vec![format!("{} 0 R", form_parent(s))]
        } else {
            (1..=n)
                .map(|i| format!("{} 0 R", page_obj(i) + 2))
                .collect()
        };
        catalog += &format!(
            " /AcroForm << /Fields [{}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >>",
            fields.join(" ")
        );
    }
    if s.dests {
        let names: Vec<String> = (1..=n)
            .map(|i| format!("({}) [{} 0 R /Fit]", dest_name(&s.tag, i), page_obj(i)))
            .collect();
        catalog += &format!(" /Names << /Dests << /Names [{}] >> >>", names.join(" "));
    }
    if s.labels {
        catalog += " /PageLabels << /Nums [0 << /S /r >> 2 << /S /D /P (A-) /St 5 >>] >>";
    }
    catalog += " >>";
    b.set(1, catalog);
    let kids: Vec<String> = (1..=n).map(|i| format!("{} 0 R", page_obj(i))).collect();
    b.set(
        2,
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", kids.join(" ")),
    );
    b.set(
        3,
        format!("<< /Producer (organise-tests) /Title ({}) >>", s.tag),
    );
    for i in 1..=n {
        let po = page_obj(i);
        let mut annots: Vec<String> = vec![];
        if s.fields {
            annots.push(format!("{} 0 R", po + 2));
        }
        if s.links {
            annots.push(format!("{} 0 R", po + 3));
            annots.push(format!("{} 0 R", po + 4));
        }
        let annots = if annots.is_empty() {
            String::new()
        } else {
            format!("/Annots [{}]", annots.join(" "))
        };
        b.set(
            po,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 4 0 R >> >> {annots} >>",
                po + 1
            ),
        );
        b.set(
            po + 1,
            stream(
                &format!("BT /F1 12 Tf 72 720 Td ({}) Tj ET", page_text(&s.tag, i)),
                "",
            ),
        );
        let val = field_value(&s.tag, i);
        if s.fields {
            // Merged field/widget dictionary with a real appearance.
            let ap = format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 150 20] /Resources << /Font << /Helv 4 0 R >> >> \
                 /Length {} >>\nstream\nBT /Helv 10 Tf 2 5 Td ({val}) Tj ET\nendstream",
                format!("BT /Helv 10 Tf 2 5 Td ({val}) Tj ET").len() + 1
            );
            // The appearance stream is inlined as an extra object after the outline objects.
            let ap_num = ap_obj(s, i);
            b.set(ap_num, ap);
            b.set(
                po + 2,
                format!(
                    "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({}) /V ({val}) /Rect [72 600 222 620] \
                     /F 4 /P {po} 0 R /DA (/Helv 10 Tf 0 g) /AP << /N {ap_num} 0 R >> {parent} >>",
                    field_name(i),
                    parent = if s.hier { format!("/Parent {} 0 R", form_parent(s)) } else { String::new() }
                ),
            );
        }
        if s.links {
            let next = i % n + 1;
            b.set(
                po + 3,
                format!(
                    "<< /Type /Annot /Subtype /Link /Rect [72 500 200 520] /Border [0 0 0] /P {po} 0 R \
                     /Dest [{} 0 R /Fit] >>",
                    page_obj(next)
                ),
            );
            b.set(
                po + 4,
                format!(
                    "<< /Type /Annot /Subtype /Link /Rect [72 400 200 420] /Border [0 0 0] /P {po} 0 R \
                     /A << /S /GoTo /D ({}) >> >>",
                    dest_name(&s.tag, next)
                ),
            );
        }
    }
    if s.outline {
        outline_objects(s, &mut b, outlines_root);
    }
    if s.fields && s.hier {
        let kids: Vec<String> = (1..=n)
            .map(|i| format!("{} 0 R", page_obj(i) + 2))
            .collect();
        b.set(
            form_parent(s),
            format!("<< /T (form1) /FT /Tx /Kids [{}] >>", kids.join(" ")),
        );
    }
    assemble(b.bodies)
}

pub fn dest_name(tag: &str, i: usize) -> String {
    format!("{tag}.dest{i}")
}

fn form_parent(s: &Spec) -> usize {
    ap_obj(s, s.pages) + 1
}

fn ap_obj(s: &Spec, i: usize) -> usize {
    let mut base = FIRST_PAGE_OBJ + PER_PAGE * s.pages;
    if s.outline {
        base += 1 + outline_count(s.pages);
    }
    base + i - 1
}

/// Chapters on odd pages; each chapter has a child "Section" on the next page (if any), the
/// section using the named destination.
fn outline_count(n: usize) -> usize {
    n
}

fn outline_objects(s: &Spec, b: &mut B, root: usize) {
    let n = s.pages;
    // item objects: root+1 ... root+n, one per page (chapter on odd pages, section on even)
    let item = |i: usize| root + i;
    let chapters: Vec<usize> = (1..=n).step_by(2).collect();
    let mut ch_dicts = vec![];
    for (ci, &p) in chapters.iter().enumerate() {
        let me = item(p);
        let mut d = format!(
            "/Title ({}) /Parent {root} 0 R /Dest [{} 0 R /Fit]",
            outline_title(&s.tag, "Chapter", p),
            page_obj(p)
        );
        if ci > 0 {
            d += &format!(" /Prev {} 0 R", item(chapters[ci - 1]));
        }
        if ci + 1 < chapters.len() {
            d += &format!(" /Next {} 0 R", item(chapters[ci + 1]));
        }
        if p < n {
            d += &format!(" /First {0} 0 R /Last {0} 0 R /Count 1", item(p + 1));
            // the section
            b.set(
                item(p + 1),
                format!(
                    "<< /Title ({}) /Parent {me} 0 R /Dest ({}) >>",
                    outline_title(&s.tag, "Section", p + 1),
                    dest_name(&s.tag, p + 1)
                ),
            );
        }
        ch_dicts.push((me, d));
    }
    for (me, d) in ch_dicts {
        b.set(me, format!("<< {d} >>"));
    }
    b.set(
        root,
        format!(
            "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
            item(chapters[0]),
            item(*chapters.last().unwrap()),
            n
        ),
    );
}

fn assemble(bodies: Vec<String>) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        let body = if b.is_empty() { "null" } else { b };
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let n = bodies.len() + 1;
    out.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R \
             /ID [<aabbccddeeff00112233445566778899> <aabbccddeeff00112233445566778899>] >>\n\
             startxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
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

/// What a document says about its navigation structure, all page references resolved to
/// zero-based page indices.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub pages: usize,
    /// `(depth, title, page)`; `page` is resolved through named destinations when needed.
    pub outline: Vec<(usize, String, Option<usize>)>,
    /// `(qualified name, value, widget pages)` in field order.
    pub fields: Vec<(String, String, Vec<Option<usize>>)>,
    pub dests: BTreeMap<String, Option<usize>>,
    pub labels: Vec<String>,
    /// Per page: link targets as page indices (explicit or via a name).
    pub links: Vec<Vec<Option<usize>>>,
    pub texts: Vec<String>,
}

pub fn summarize(doc: &Document) -> Summary {
    let n = doc.page_count().unwrap();
    let page_index = |o: &papyrine_cos::Object| -> Option<usize> {
        if o.kind().ok()? != ObjectKind::Dictionary {
            return None;
        }
        doc.find_page(o).ok()
    };
    let dest_page = |d: &papyrine_cos::Object| -> Option<usize> {
        let d = if d.kind().ok()? == ObjectKind::Dictionary {
            d.dict_get("D").ok()?
        } else {
            d.clone()
        };
        if d.kind().ok()? == ObjectKind::Array {
            return page_index(&d.array_get(0).ok()?);
        }
        None
    };
    let mut dests = BTreeMap::new();
    if let Some(tree) = doc.catalog_name_tree("Dests", false).unwrap() {
        for k in doc.name_tree_keys(&tree).unwrap() {
            let v = doc.name_tree_get(&tree, &k).unwrap().unwrap();
            dests.insert(String::from_utf8_lossy(&k).into_owned(), dest_page(&v));
        }
    }
    let resolve_name = |name: &[u8]| -> Option<usize> {
        let tree = doc.catalog_name_tree("Dests", false).ok()??;
        dest_page(&doc.name_tree_get(&tree, name).ok()??)
    };
    let outline = doc
        .outlines()
        .unwrap()
        .into_iter()
        .map(|o| {
            let page = o.page.or_else(|| {
                o.dest_name
                    .as_ref()
                    .and_then(|d| resolve_name(d.as_bytes()))
            });
            (o.depth, o.title, page)
        })
        .collect();
    let fields = doc
        .form_fields()
        .unwrap()
        .into_iter()
        .map(|f| (f.name, f.value, f.widgets.iter().map(|w| w.page).collect()))
        .collect();
    let labels = {
        let pl = doc.page_labels().unwrap();
        (0..n).map(|i| pl.label_for(i)).collect()
    };
    let mut links = vec![];
    let mut texts = vec![];
    for i in 0..n {
        let p = doc.page(i).unwrap();
        let mut l = vec![];
        let annots = p.dict_get("Annots").unwrap();
        if annots.kind().unwrap() == ObjectKind::Array {
            for a in annots.array_items().unwrap() {
                if a.kind().unwrap() != ObjectKind::Dictionary
                    || a.dict_get("Subtype").unwrap().name().unwrap_or_default() != b"Link"
                {
                    continue;
                }
                let d = a.dict_get("Dest").unwrap();
                if d.kind().unwrap() == ObjectKind::Array {
                    l.push(dest_page(&d));
                } else if d.kind().unwrap() == ObjectKind::String {
                    l.push(resolve_name(&d.string().unwrap()));
                } else {
                    let act = a.dict_get("A").unwrap();
                    let dd = act.dict_get("D").unwrap();
                    if dd.kind().unwrap() == ObjectKind::String {
                        l.push(resolve_name(&dd.string().unwrap()));
                    } else {
                        l.push(dest_page(&dd));
                    }
                }
            }
        }
        links.push(l);
        let c = p.dict_get("Contents").unwrap();
        let bytes = c
            .stream_decoded(papyrine_cos::DecodeLevel::Generalized)
            .map(|b| b.to_vec())
            .unwrap_or_default();
        let t = String::from_utf8_lossy(&bytes).into_owned();
        texts.push(t.split(['(', ')']).nth(1).unwrap_or("").to_string());
    }
    Summary {
        pages: n,
        outline,
        fields,
        dests,
        labels,
        links,
        texts,
    }
}

// ---------------------------------------------------------------------------------------------
// Interop oracles: qpdf --check, Poppler (pdfinfo, pdftotext) and PDFium. Missing tools are
// skipped with a note unless PAPYRINE_REQUIRE_ORACLES is set (CI sets it).

use std::process::Command;
use std::sync::OnceLock;

fn tool(name: &str, arg: &str) -> bool {
    Command::new(name)
        .arg(arg)
        .output()
        .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
        .unwrap_or(false)
}

fn require() -> bool {
    std::env::var_os("PAPYRINE_REQUIRE_ORACLES").is_some()
}

fn have(name: &str, arg: &str) -> bool {
    let ok = tool(name, arg);
    if !ok {
        assert!(
            !require(),
            "{name} is required (PAPYRINE_REQUIRE_ORACLES) but not found"
        );
        eprintln!("note: {name} not found; skipping that interop check");
    }
    ok
}

pub fn tmp_pdf(bytes: &[u8]) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
    f
}

/// `qpdf --check` must report nothing at all (exit 0: no errors, no warnings).
pub fn qpdf_check(bytes: &[u8]) -> Result<(), String> {
    if !have("qpdf", "--version") {
        return Ok(());
    }
    let f = tmp_pdf(bytes);
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(f.path())
        .output()
        .unwrap();
    if out.status.code() == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "qpdf --check exit {:?}: {}{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Poppler: page count from `pdfinfo` and each page's text from `pdftotext`.
pub fn poppler_check(bytes: &[u8], texts: &[String]) -> Result<(), String> {
    if !have("pdfinfo", "-v") || !have("pdftotext", "-v") {
        return Ok(());
    }
    let f = tmp_pdf(bytes);
    let info = Command::new("pdfinfo").arg(f.path()).output().unwrap();
    let info = String::from_utf8_lossy(&info.stdout).into_owned();
    let pages: usize = info
        .lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| format!("pdfinfo gave no page count: {info}"))?;
    if pages != texts.len() {
        return Err(format!("pdfinfo: {pages} pages, expected {}", texts.len()));
    }
    let out = Command::new("pdftotext")
        .arg(f.path())
        .arg("-")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let per_page: Vec<&str> = text.split('\x0c').collect();
    for (i, want) in texts.iter().enumerate() {
        if want.is_empty() {
            continue;
        }
        if !per_page.get(i).is_some_and(|p| p.contains(want.as_str())) {
            return Err(format!("pdftotext: page {} lacks {want:?}", i + 1));
        }
    }
    Ok(())
}

fn pdfium_lib() -> Option<std::sync::Arc<papyrine_render::Library>> {
    static LIB: OnceLock<Option<std::sync::Arc<papyrine_render::Library>>> = OnceLock::new();
    LIB.get_or_init(|| {
        let l = papyrine_render::Library::global().ok();
        if l.is_none() {
            assert!(
                !require(),
                "PDFium is required (PAPYRINE_REQUIRE_ORACLES) but not found"
            );
            eprintln!("note: PDFium not found; skipping that interop check");
        }
        l
    })
    .clone()
}

/// PDFium opens the file, sees the pages, extracts each page's text and renders it with ink.
pub fn pdfium_check(bytes: &[u8], texts: &[String]) -> Result<(), String> {
    let Some(lib) = pdfium_lib() else {
        return Ok(());
    };
    let mut d = papyrine_render::Document::open(
        &lib,
        papyrine_render::bytes_from_vec(bytes.to_vec()),
        &[],
        None,
    )
    .map_err(|e| format!("PDFium open: {e}"))?;
    if d.page_count() != texts.len() {
        return Err(format!(
            "PDFium: {} pages, expected {}",
            d.page_count(),
            texts.len()
        ));
    }
    for (i, want) in texts.iter().enumerate() {
        let t = d
            .page_text(i)
            .map_err(|e| format!("PDFium text p{}: {e}", i + 1))?;
        if !want.is_empty() && !t.text.contains(want.as_str()) {
            return Err(format!(
                "PDFium: page {} text {:?} lacks {want:?}",
                i + 1,
                t.text
            ));
        }
        let tile = d
            .render_preview(i, 400, &mut || false)
            .map_err(|e| format!("PDFium render p{}: {e}", i + 1))?;
        let ink = tile
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] < 245)
            .count();
        if !want.is_empty() && ink == 0 {
            return Err(format!("PDFium: page {} rendered blank", i + 1));
        }
    }
    Ok(())
}

/// All three oracles on one output. `texts` is the expected text of each page ("" = blank).
pub fn interop(bytes: &[u8], texts: &[String]) {
    let mut errs = vec![];
    for r in [
        qpdf_check(bytes),
        poppler_check(bytes, texts),
        pdfium_check(bytes, texts),
    ] {
        if let Err(e) = r {
            errs.push(e);
        }
    }
    assert!(errs.is_empty(), "interop failures:\n{}", errs.join("\n"));
}
