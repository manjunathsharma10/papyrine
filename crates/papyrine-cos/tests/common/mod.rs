#![allow(dead_code)]

use papyrine_cos::{Document, EncryptionMode, EncryptionSpec, OpenOptions, WriteOptions};

pub const PAGE_TEXT_PREFIX: &str = "BT /F1 12 Tf 72 720 Td (Page ";

pub fn content_for(i: usize) -> String {
    format!("{PAGE_TEXT_PREFIX}{i}) Tj ET\n")
}

/// A valid classic-xref PDF with `pages` pages, built byte by byte.
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
        "<< /Producer (papyrine-tests) /Title (Test document) >>".into(),
    );
    for i in 0..pages {
        let content = content_for(i + 1);
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

pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

pub fn replace_first(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let p = find(hay, from).expect("pattern present");
    let mut v = hay[..p].to_vec();
    v.extend_from_slice(to);
    v.extend_from_slice(&hay[p + from.len()..]);
    v
}

/// A tiny deterministic PRNG so mutation tests are reproducible.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

/// Build a one-page encrypted PDF with the library's own writer.
pub fn encrypted_pdf(spec: EncryptionSpec) -> Vec<u8> {
    let plain = Document::open_bytes(build_pdf(2), &OpenOptions::default()).unwrap();
    let opts = WriteOptions {
        encryption: EncryptionMode::Encrypt(spec),
        static_id: true,
        ..WriteOptions::default()
    };
    plain.write(&opts).unwrap().into_vec()
}

pub fn decoded_content(doc: &Document, page: usize) -> Vec<u8> {
    let p = doc.page(page).unwrap();
    let c = p.dict_get("Contents").unwrap();
    c.stream_decoded(papyrine_cos::DecodeLevel::Generalized)
        .unwrap()
        .to_vec()
}
