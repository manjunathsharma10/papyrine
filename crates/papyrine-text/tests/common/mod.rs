#![allow(dead_code)]
use papyrine_core::{CancelToken, Rect};
use papyrine_text::*;

/// Lay `text` out on a grid: 10pt per char, 14pt per line (newline chars get an
/// empty box like PDFium's generated ones). Lines are numbered from the top.
pub fn page(text: &str) -> PageInput {
    let (mut x, mut line) = (0.0, 0.0);
    let mut chars = Vec::new();
    let mut boxes = Vec::new();
    for c in text.chars() {
        chars.push(c);
        if c == '\n' || c == '\r' {
            boxes.push(Rect::new(x, 800.0 - line * 14.0, x, 800.0 - line * 14.0));
            if c == '\n' {
                x = 0.0;
                line += 1.0;
            }
            continue;
        }
        boxes.push(Rect::new(
            x,
            800.0 - line * 14.0 - 12.0,
            x + 10.0,
            800.0 - line * 14.0,
        ));
        x += 10.0;
    }
    PageInput {
        page: 0,
        text: PageChars::new(chars, boxes),
        extras: vec![],
    }
}

pub fn run(text: &str, q: &str, o: &SearchOptions) -> Vec<SearchHit> {
    run_pages(vec![page(text)], q, o)
}

pub fn run_pages(pages: Vec<PageInput>, q: &str, o: &SearchOptions) -> Vec<SearchHit> {
    let mut hits = vec![];
    search_document(pages, q, o, &CancelToken::new(), |e| {
        if let SearchEvent::Hit(h) = e {
            hits.push(h)
        }
    });
    hits
}

/// The original text covered by each hit.
pub fn matched(text: &str, q: &str, o: &SearchOptions) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    run(text, q, o)
        .into_iter()
        .map(|h| chars[h.range].iter().collect())
        .collect()
}

pub fn opts() -> SearchOptions {
    SearchOptions::default()
}

/// Deterministic xorshift for reproducible generated pages.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// `n` pages of ~`chars` chars each: Latin words with accents and ligatures,
/// Cyrillic, and the odd Arabic/CJK word, wrapped at 80 columns with `\r\n`.
/// `needle` is planted on `needle_pages`.
pub fn generate(n: usize, chars: usize, needle: &str, needle_pages: &[usize]) -> Vec<PageInput> {
    const WORDS: &[&str] = &[
        "the",
        "quick",
        "brown",
        "fox",
        "über",
        "naïve",
        "café",
        "e\u{fb03}cient",
        "o\u{fb00}ice",
        "document",
        "page",
        "reader",
        "Привет",
        "мир",
        "ملف",
        "مرحبا",
        "日本語",
        "テキスト",
        "search",
        "hyphen\u{ad}ation",
        "Straße",
        "résumé",
        "binary",
        "tree",
        "stream",
        "object",
        "xref",
        "trailer",
        "content",
        "annotation",
    ];
    let mut rng = Rng(0x9e3779b97f4a7c15);
    (0..n)
        .map(|p| {
            let mut s = String::with_capacity(chars + 64);
            let mut col = 0;
            let mut planted = !needle_pages.contains(&p);
            while s.chars().count() < chars {
                let w = if !planted && s.len() > chars / 2 {
                    planted = true;
                    needle
                } else {
                    WORDS[(rng.next() % WORDS.len() as u64) as usize]
                };
                if col + w.chars().count() > 80 {
                    s.push_str("\r\n");
                    col = 0;
                }
                s.push_str(w);
                s.push(' ');
                col += w.chars().count() + 1;
            }
            let mut inp = page(&s.replace("\r\n", "\n"));
            inp.page = p;
            inp
        })
        .collect()
}
