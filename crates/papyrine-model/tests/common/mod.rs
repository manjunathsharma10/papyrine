#![allow(dead_code)]

use papyrine_cos::OpenOptions;
use papyrine_model::Model;

/// Build a classic-xref PDF from object bodies; `objs[i]` is object `i + 1`. Object 1 must be the
/// catalog. `trailer_extra` is appended inside the trailer dictionary (e.g. `/Info 9 0 R`).
pub fn build(objs: &[String], trailer_extra: &str) -> Vec<u8> {
    build_versioned("1.7", objs, trailer_extra)
}

pub fn build_versioned(version: &str, objs: &[String], trailer_extra: &str) -> Vec<u8> {
    let mut out: Vec<u8> = format!("%PDF-{version}\n%\u{e2}\u{e3}\u{cf}\u{d3}\n").into_bytes();
    // The binary comment above is UTF-8 in this string; that is fine for a marker line.
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let n = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {n}\n0000000000 65535 f \n").as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {n} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

pub fn stream(dict_extra: &str, data: &str) -> String {
    format!(
        "<< /Length {} {dict_extra} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

pub fn model(pdf: Vec<u8>) -> Model {
    Model::open_bytes(pdf, &OpenOptions::default()).expect("open generated pdf")
}

pub fn page_obj(extra: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {extra} >>")
}

/// Catalog + Pages with `n` plain pages as objects 3.. ; extra catalog entries and extra objects
/// (numbered from `3 + n`) can be supplied.
pub fn simple_doc(n: usize, catalog_extra: &str, extra_objs: &[String]) -> Vec<String> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", kids.join(" ")),
    ];
    for _ in 0..n {
        objs.push(page_obj(""));
    }
    objs.extend(extra_objs.iter().cloned());
    objs
}
