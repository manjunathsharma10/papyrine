use qpdf_sys as ffi;

use crate::{Document, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// Prefix only, no number.
    None,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl LabelStyle {
    fn code(self) -> i32 {
        match self {
            LabelStyle::None => 0,
            LabelStyle::Decimal => 1,
            LabelStyle::LowerAlpha => 2,
            LabelStyle::UpperAlpha => 3,
            LabelStyle::LowerRoman => 4,
            LabelStyle::UpperRoman => 5,
        }
    }

    fn from_code(c: i32) -> Self {
        match c {
            1 => LabelStyle::Decimal,
            2 => LabelStyle::LowerAlpha,
            3 => LabelStyle::UpperAlpha,
            4 => LabelStyle::LowerRoman,
            5 => LabelStyle::UpperRoman,
            _ => LabelStyle::None,
        }
    }
}

/// A run of pages numbered the same way, starting at `start_page` (zero-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLabelRange {
    pub start_page: usize,
    pub style: LabelStyle,
    pub prefix: String,
    /// Number shown on the first page of the range (`/St`, at least 1).
    pub first_value: u32,
}

/// All label ranges of a document, sorted by start page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageLabels {
    pub ranges: Vec<PageLabelRange>,
}

impl PageLabels {
    /// The label text shown for a zero-based page index. Without any range the label is the
    /// decimal page number, as in viewers.
    pub fn label_for(&self, page: usize) -> String {
        let Some(r) = self.ranges.iter().rev().find(|r| r.start_page <= page) else {
            return (page + 1).to_string();
        };
        let n = r.first_value as usize + (page - r.start_page);
        let body = match r.style {
            LabelStyle::None => String::new(),
            LabelStyle::Decimal => n.to_string(),
            LabelStyle::LowerAlpha => alpha(n, false),
            LabelStyle::UpperAlpha => alpha(n, true),
            LabelStyle::LowerRoman => roman(n).to_lowercase(),
            LabelStyle::UpperRoman => roman(n),
        };
        format!("{}{}", r.prefix, body)
    }
}

/// a..z, aa..zz, aaa.. (the PDF scheme repeats the letter, it is not base 26).
fn alpha(n: usize, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let letter = (b'a' + ((n - 1) % 26) as u8) as char;
    let s: String = std::iter::repeat_n(letter, (n - 1) / 26 + 1).collect();
    if upper { s.to_uppercase() } else { s }
}

fn roman(mut n: usize) -> String {
    const T: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
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

impl Document {
    pub fn page_labels(&self) -> Result<PageLabels> {
        Ok(PageLabels {
            ranges: ffi::page_labels_read(self.ffi())?
                .into_iter()
                .map(|r| PageLabelRange {
                    start_page: r.start_page.max(0) as usize,
                    style: LabelStyle::from_code(r.style),
                    prefix: String::from_utf8_lossy(&r.prefix).into_owned(),
                    first_value: r.first_value.max(1) as u32,
                })
                .collect(),
        })
    }

    /// Replace the page labels. Ranges must be sorted by distinct start pages and the first must
    /// start at page 0 (a PDF requirement); an empty list removes the labels.
    pub fn set_page_labels(&self, labels: &PageLabels) -> Result<()> {
        let r = &labels.ranges;
        if let Some(first) = r.first()
            && first.start_page != 0
        {
            return Err(Error::Range(
                "first page label range must start at page 0".into(),
            ));
        }
        if r.windows(2).any(|w| w[0].start_page >= w[1].start_page) {
            return Err(Error::Range(
                "page label ranges must have strictly increasing start pages".into(),
            ));
        }
        let raw: Vec<ffi::LabelRange> = r
            .iter()
            .map(|x| ffi::LabelRange {
                start_page: x.start_page as i32,
                style: x.style.code(),
                prefix: x.prefix.clone().into_bytes(),
                first_value: x.first_value.max(1) as i32,
            })
            .collect();
        Ok(ffi::page_labels_write(self.ffi(), &raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(alpha(1, false), "a");
        assert_eq!(alpha(27, true), "AA");
        assert_eq!(alpha(53, false), "aaa");
    }

    #[test]
    fn label_for_ranges() {
        let l = PageLabels {
            ranges: vec![
                PageLabelRange {
                    start_page: 0,
                    style: LabelStyle::LowerRoman,
                    prefix: String::new(),
                    first_value: 1,
                },
                PageLabelRange {
                    start_page: 3,
                    style: LabelStyle::Decimal,
                    prefix: "A-".into(),
                    first_value: 7,
                },
            ],
        };
        assert_eq!(l.label_for(2), "iii");
        assert_eq!(l.label_for(3), "A-7");
        assert_eq!(l.label_for(5), "A-9");
        assert_eq!(PageLabels::default().label_for(4), "5");
    }
}
