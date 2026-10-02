//! Minimal PDF generator for tests and benches (never writes files itself).
//! Classic xref tables, uncompressed streams, one Helvetica font.

use std::fmt::Write as _;

pub struct PageSpec {
    pub width: f32,
    pub height: f32,
    pub rotate: i32,
    pub content: Vec<u8>,
}

impl PageSpec {
    pub fn letter(content: impl Into<Vec<u8>>) -> Self {
        PageSpec {
            width: 612.0,
            height: 792.0,
            rotate: 0,
            content: content.into(),
        }
    }
}

/// A generated base file plus what is needed to chain incremental updates.
pub struct Built {
    pub bytes: Vec<u8>,
    pub startxref: usize,
    /// Next free object number (== /Size).
    pub size: u32,
    pub root: u32,
    pub page_ids: Vec<u32>,
    pub content_ids: Vec<u32>,
}

fn obj(id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = format!("{id} 0 obj\n").into_bytes();
    v.extend_from_slice(body);
    v.extend_from_slice(b"\nendobj\n");
    v
}

pub fn stream_body(extra_dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< /Length {} {extra_dict}>>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

/// Object ids: 1 catalog, 2 pages, 3 font, then (page, content) pairs from 4.
pub fn build(pages: &[PageSpec]) -> Built {
    let n = pages.len() as u32;
    let size = 4 + 2 * n;
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize; size as usize];
    let mut put = |out: &mut Vec<u8>, id: u32, body: &[u8]| {
        offsets[id as usize] = out.len();
        out.extend_from_slice(&obj(id, body));
    };
    put(&mut out, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..n).fold(String::new(), |mut s, i| {
        let _ = write!(s, "{} 0 R ", 4 + 2 * i);
        s
    });
    put(
        &mut out,
        2,
        format!("<< /Type /Pages /Count {n} /Kids [{kids}] >>").as_bytes(),
    );
    put(
        &mut out,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let mut page_ids = Vec::new();
    let mut content_ids = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let (pid, cid) = (4 + 2 * i as u32, 5 + 2 * i as u32);
        page_ids.push(pid);
        content_ids.push(cid);
        put(&mut out, pid, page_dict(p, cid).as_bytes());
        put(&mut out, cid, &stream_body("", &p.content));
    }
    let startxref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in &offsets[1..] {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n")
            .as_bytes(),
    );
    Built {
        bytes: out,
        startxref,
        size,
        root: 1,
        page_ids,
        content_ids,
    }
}

fn page_dict(p: &PageSpec, content_id: u32) -> String {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Rotate {} /Contents {content_id} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        p.width, p.height, p.rotate
    )
}

/// Replacement page-dict + content objects for `build`'s page `i`.
pub fn page_objects(b: &Built, i: usize, spec: &PageSpec) -> Vec<(u32, Vec<u8>)> {
    vec![(b.content_ids[i], stream_body("", &spec.content))]
}

/// Builds a chain of incremental-update sections that append to a base file.
#[derive(Clone)]
pub struct Chain {
    pub total_len: usize,
    pub startxref: usize,
    pub size: u32,
    pub root: u32,
}

impl Chain {
    pub fn new(base: &Built) -> Self {
        Chain {
            total_len: base.bytes.len(),
            startxref: base.startxref,
            size: base.size,
            root: base.root,
        }
    }

    /// Allocate a fresh object id (for new objects in the next section).
    pub fn alloc_id(&mut self) -> u32 {
        self.size += 1;
        self.size - 1
    }

    /// Serialise one incremental section containing `objs` (id, body) and
    /// advance the chain. Offsets are absolute in the concatenated file.
    pub fn section(&mut self, objs: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut objs: Vec<&(u32, Vec<u8>)> = objs.iter().collect();
        objs.sort_by_key(|o| o.0);
        let mut v = b"\n".to_vec();
        let mut offs = Vec::new();
        for (id, body) in &objs {
            offs.push((*id, self.total_len + v.len()));
            v.extend_from_slice(&obj(*id, body));
        }
        let xref_abs = self.total_len + v.len();
        v.extend_from_slice(b"xref\n");
        let mut i = 0;
        while i < offs.len() {
            let mut j = i + 1;
            while j < offs.len() && offs[j].0 == offs[j - 1].0 + 1 {
                j += 1;
            }
            v.extend_from_slice(format!("{} {}\n", offs[i].0, j - i).as_bytes());
            for (_, off) in &offs[i..j] {
                v.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
            }
            i = j;
        }
        v.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {} 0 R /Prev {} >>\nstartxref\n{xref_abs}\n%%EOF\n",
                self.size, self.root, self.startxref
            )
            .as_bytes(),
        );
        self.total_len += v.len();
        self.startxref = xref_abs;
        v
    }
}

/// Small deterministic PRNG (xorshift64*), for reproducible bulk content.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Page content: text lines plus `rects` small filled rectangles.
pub fn busy_content(page_no: usize, lines: usize, rects: usize, rng: &mut Rng) -> Vec<u8> {
    const WORDS: [&str; 12] = [
        "lorem", "ipsum", "dolor", "sit", "amet", "papyrine", "render", "tile", "page", "buffer",
        "section", "xref",
    ];
    let mut s = String::with_capacity(lines * 100 + rects * 32);
    s.push_str("BT /F1 10 Tf 12 TL 36 756 Td\n");
    for l in 0..lines {
        let mut line = format!("Page {page_no} line {l}:");
        while line.len() < 85 {
            line.push(' ');
            line.push_str(WORDS[(rng.next_u64() % 12) as usize]);
        }
        let _ = writeln!(s, "({line}) Tj T*");
    }
    s.push_str("ET\n");
    for _ in 0..rects {
        let r = rng.next_u64();
        let _ = writeln!(
            s,
            "{:.2} {:.2} {:.2} rg {} {} {} {} re f",
            (r & 0xff) as f32 / 255.0,
            ((r >> 8) & 0xff) as f32 / 255.0,
            ((r >> 16) & 0xff) as f32 / 255.0,
            20 + (r >> 24) % 540,
            20 + (r >> 34) % 700,
            2 + (r >> 44) % 18,
            2 + (r >> 52) % 18
        );
    }
    s.into_bytes()
}

/// Content: "Hello Papyrine" at (72,700) 24pt, black square 100..300 (x and y,
/// page space), then nothing else. Device y of the square at scale 1 on a
/// 792-pt page: 492..692.
pub fn hello_content() -> Vec<u8> {
    b"0 0 0 rg 100 100 200 200 re f\nBT /F1 24 Tf 72 700 Td (Hello Papyrine) Tj ET\n".to_vec()
}

/// Same square in pure red (used by the incremental-update test).
pub fn red_content() -> Vec<u8> {
    b"1 0 0 rg 100 100 200 200 re f\nBT /F1 24 Tf 72 700 Td (Hello Papyrine) Tj ET\n".to_vec()
}

/// A page with an unreferenced filler stream object of `n` bytes, for sizing sections.
pub fn filler_body(n: usize, rng: &mut Rng) -> Vec<u8> {
    let mut data = Vec::with_capacity(n);
    while data.len() < n {
        data.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    data.truncate(n);
    stream_body("", &data)
}
