//! Page selection: all, current, or a typed range such as `1-3, 5, 8-`.

use crate::Error;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageSelection {
    #[default]
    All,
    /// Zero-based page index of the page being viewed.
    Current { index: usize },
    /// One-based ranges as typed in the dialog, e.g. `"1-3, 5, 8-"`.
    Ranges { text: String },
}

impl PageSelection {
    /// Zero-based page indices in ascending order without duplicates.
    pub fn resolve(&self, page_count: usize) -> Result<Vec<usize>, Error> {
        if page_count == 0 {
            return Err(Error::EmptySelection);
        }
        let pages: Vec<usize> = match self {
            PageSelection::All => (0..page_count).collect(),
            PageSelection::Current { index } => {
                if *index >= page_count {
                    return Err(Error::PageOutOfRange {
                        page: index + 1,
                        count: page_count,
                    });
                }
                vec![*index]
            }
            PageSelection::Ranges { text } => parse_ranges(text, page_count)?,
        };
        if pages.is_empty() {
            return Err(Error::EmptySelection);
        }
        Ok(pages)
    }
}

/// Parse `1-3, 5, 8-` (one-based; `-N` means 1..=N, `N-` means N..=last) against `page_count`.
pub fn parse_ranges(text: &str, page_count: usize) -> Result<Vec<usize>, Error> {
    let bad = |s: &str| Error::BadRange(s.trim().to_string());
    let mut keep = vec![false; page_count];
    for part in text.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| -> Result<usize, Error> {
            let n: usize = s.trim().parse().map_err(|_| bad(part))?;
            if n == 0 {
                return Err(bad(part));
            }
            if n > page_count {
                return Err(Error::PageOutOfRange {
                    page: n,
                    count: page_count,
                });
            }
            Ok(n)
        };
        let (lo, hi) = match part.split_once('-') {
            None => {
                let n = num(part)?;
                (n, n)
            }
            Some((a, b)) => {
                let lo = if a.trim().is_empty() { 1 } else { num(a)? };
                let hi = if b.trim().is_empty() {
                    page_count
                } else {
                    num(b)?
                };
                if lo > hi {
                    return Err(bad(part));
                }
                (lo, hi)
            }
        };
        for k in &mut keep[lo - 1..hi] {
            *k = true;
        }
    }
    Ok(keep
        .iter()
        .enumerate()
        .filter_map(|(i, k)| k.then_some(i))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists_ranges_and_open_ends() {
        assert_eq!(parse_ranges("1-3, 5", 10).unwrap(), vec![0, 1, 2, 4]);
        assert_eq!(parse_ranges("8-", 10).unwrap(), vec![7, 8, 9]);
        assert_eq!(parse_ranges("-2", 10).unwrap(), vec![0, 1]);
        assert_eq!(parse_ranges("3,3,2-3", 10).unwrap(), vec![1, 2]);
        assert_eq!(parse_ranges(" 4 ; 6 ", 10).unwrap(), vec![3, 5]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(parse_ranges("0", 5), Err(Error::BadRange(_))));
        assert!(matches!(parse_ranges("a-b", 5), Err(Error::BadRange(_))));
        assert!(matches!(parse_ranges("4-2", 5), Err(Error::BadRange(_))));
        assert!(matches!(
            parse_ranges("6", 5),
            Err(Error::PageOutOfRange { page: 6, count: 5 })
        ));
    }

    #[test]
    fn selections_resolve() {
        assert_eq!(PageSelection::All.resolve(3).unwrap(), vec![0, 1, 2]);
        assert_eq!(
            PageSelection::Current { index: 1 }.resolve(3).unwrap(),
            vec![1]
        );
        assert!(PageSelection::Current { index: 3 }.resolve(3).is_err());
        assert!(matches!(
            PageSelection::Ranges { text: " , ".into() }.resolve(3),
            Err(Error::EmptySelection)
        ));
        assert!(matches!(
            PageSelection::All.resolve(0),
            Err(Error::EmptySelection)
        ));
    }
}
