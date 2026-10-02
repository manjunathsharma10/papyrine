//! Page labels (PDF 12.4.2).

use std::rc::Rc;

use papyrine_cos::Result;

use crate::model::{Key, Model};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRange {
    /// Zero-based index of the first page of the range.
    pub start_index: usize,
    /// `None` means no numeric part (prefix only).
    pub style: Option<LabelStyle>,
    pub prefix: String,
    /// `/St`, at least 1.
    pub first_number: u32,
}

#[derive(Debug, Clone, Default)]
pub struct PageLabels {
    /// Sorted by `start_index`.
    pub ranges: Vec<LabelRange>,
}

impl PageLabels {
    /// Does the document define labels at all?
    pub fn is_defined(&self) -> bool {
        !self.ranges.is_empty()
    }

    /// The label shown for page `index`; plain decimal page numbers when none are defined.
    pub fn label(&self, index: usize) -> String {
        let Some(range) = self.ranges.iter().rev().find(|r| r.start_index <= index) else {
            return (index + 1).to_string();
        };
        let n = u64::from(range.first_number) + (index - range.start_index) as u64;
        let mut s = range.prefix.clone();
        if let Some(style) = range.style {
            s.push_str(&format_number(n, style));
        }
        s
    }

    /// First page whose label equals `label` (case-sensitive).
    pub fn find(&self, label: &str, page_count: usize) -> Option<usize> {
        (0..page_count).find(|&i| self.label(i) == label)
    }
}

pub fn format_number(n: u64, style: LabelStyle) -> String {
    match style {
        LabelStyle::Decimal => n.to_string(),
        LabelStyle::UpperRoman => roman(n).to_uppercase(),
        LabelStyle::LowerRoman => roman(n),
        LabelStyle::UpperAlpha => alpha(n, true),
        LabelStyle::LowerAlpha => alpha(n, false),
    }
}

fn roman(mut n: u64) -> String {
    if n == 0 || n > 100_000 {
        return if n == 0 { String::new() } else { n.to_string() };
    }
    const T: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// A..Z, then AA..ZZ, AAA..: the letter cycles and the repeat count grows.
fn alpha(n: u64, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let letter = ((n - 1) % 26) as u8;
    let reps = ((n - 1) / 26 + 1).min(1000) as usize;
    let c = (if upper { b'A' } else { b'a' } + letter) as char;
    std::iter::repeat_n(c, reps).collect()
}

impl Model {
    pub fn page_labels(&self) -> Result<Rc<PageLabels>> {
        self.cached(Key::PageLabels, |m| {
            let mut ranges = Vec::new();
            if let Some(root) = m.root()
                && let Some(tree) = m.get(&root, "PageLabels")
            {
                for (start, dict) in m.number_tree_entries(&tree) {
                    if start < 0 {
                        continue;
                    }
                    let style = match m.name_of(&dict, "S").as_deref() {
                        Some("D") => Some(LabelStyle::Decimal),
                        Some("R") => Some(LabelStyle::UpperRoman),
                        Some("r") => Some(LabelStyle::LowerRoman),
                        Some("A") => Some(LabelStyle::UpperAlpha),
                        Some("a") => Some(LabelStyle::LowerAlpha),
                        _ => None,
                    };
                    ranges.push(LabelRange {
                        start_index: start as usize,
                        style,
                        prefix: m.text_of(&dict, "P").unwrap_or_default(),
                        first_number: m
                            .int_of(&dict, "St")
                            .filter(|&s| s >= 1)
                            .unwrap_or(1)
                            .min(i64::from(u32::MAX)) as u32,
                    });
                }
            }
            ranges.sort_by_key(|r| r.start_index);
            ranges.dedup_by_key(|r| r.start_index);
            Ok(PageLabels { ranges })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerals() {
        assert_eq!(format_number(1994, LabelStyle::UpperRoman), "MCMXCIV");
        assert_eq!(format_number(4, LabelStyle::LowerRoman), "iv");
        assert_eq!(format_number(26, LabelStyle::UpperAlpha), "Z");
        assert_eq!(format_number(27, LabelStyle::UpperAlpha), "AA");
        assert_eq!(format_number(53, LabelStyle::LowerAlpha), "aaa");
        assert_eq!(format_number(0, LabelStyle::LowerRoman), "");
    }

    #[test]
    fn ranges() {
        let l = PageLabels {
            ranges: vec![
                LabelRange {
                    start_index: 0,
                    style: Some(LabelStyle::LowerRoman),
                    prefix: String::new(),
                    first_number: 1,
                },
                LabelRange {
                    start_index: 3,
                    style: Some(LabelStyle::Decimal),
                    prefix: "A-".into(),
                    first_number: 5,
                },
                LabelRange {
                    start_index: 6,
                    style: None,
                    prefix: "Cover".into(),
                    first_number: 1,
                },
            ],
        };
        let got: Vec<String> = (0..8).map(|i| l.label(i)).collect();
        assert_eq!(
            got,
            ["i", "ii", "iii", "A-5", "A-6", "A-7", "Cover", "Cover"]
        );
        assert_eq!(l.find("A-6", 8), Some(4));
        assert_eq!(PageLabels::default().label(9), "10");
    }
}
