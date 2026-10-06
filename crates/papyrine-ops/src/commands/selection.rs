//! Page selections for the organise commands: "all", "odd", "even", explicit lists and ranges
//! such as `1-3, 7, 10-` (one-based, as the user types them).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Which of the selected pages to keep, by one-based page number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Parity {
    #[default]
    All,
    /// Pages 1, 3, 5, ...
    Odd,
    /// Pages 2, 4, 6, ...
    Even,
}

/// A set of pages described the way a person would.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSelection {
    /// Inclusive one-based ranges. `None` bounds are open: the first or last page.
    ranges: Vec<(Option<usize>, Option<usize>)>,
    parity: Parity,
}

impl PageSelection {
    /// Every page.
    pub fn all() -> Self {
        PageSelection {
            ranges: vec![(None, None)],
            parity: Parity::All,
        }
    }

    /// Odd (1, 3, 5, ...) or even page numbers.
    pub fn parity(parity: Parity) -> Self {
        PageSelection {
            ranges: vec![(None, None)],
            parity,
        }
    }

    /// One-based inclusive range.
    pub fn range(first: usize, last: usize) -> Self {
        PageSelection {
            ranges: vec![(Some(first), Some(last))],
            parity: Parity::All,
        }
    }

    /// Keep only odd or even page numbers of this selection.
    pub fn only(mut self, parity: Parity) -> Self {
        self.parity = parity;
        self
    }

    /// Parse `1-3,7,10-`, `-4` (up to page 4), `all`, `odd`, `even`; any list may end with
    /// `odd` or `even` (`2-20 even`). Spaces are ignored around separators.
    pub fn parse(text: &str) -> Result<Self> {
        let t = text.trim().to_ascii_lowercase();
        let (list, parity) = if let Some(l) = t.strip_suffix("odd") {
            (l.trim(), Parity::Odd)
        } else if let Some(l) = t.strip_suffix("even") {
            (l.trim(), Parity::Even)
        } else {
            (t.as_str(), Parity::All)
        };
        if list.is_empty() || list == "all" {
            if t.is_empty() {
                return Err(Error::invalid("empty page selection"));
            }
            return Ok(PageSelection {
                ranges: vec![(None, None)],
                parity,
            });
        }
        let num = |s: &str| -> Result<usize> {
            s.trim()
                .parse::<usize>()
                .ok()
                .filter(|&n| n >= 1)
                .ok_or_else(|| Error::invalid(format!("`{}` is not a page number", s.trim())))
        };
        let mut ranges = vec![];
        for part in list.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(Error::invalid("empty item in page selection"));
            }
            match part.split_once('-') {
                None => {
                    let n = num(part)?;
                    ranges.push((Some(n), Some(n)));
                }
                Some((a, b)) => {
                    let a = (!a.trim().is_empty()).then(|| num(a)).transpose()?;
                    let b = (!b.trim().is_empty()).then(|| num(b)).transpose()?;
                    if let (Some(x), Some(y)) = (a, b)
                        && x > y
                    {
                        return Err(Error::invalid(format!("range {x}-{y} runs backwards")));
                    }
                    ranges.push((a, b));
                }
            }
        }
        Ok(PageSelection { ranges, parity })
    }

    /// Zero-based indices, sorted and unique. Pages beyond `page_count` are an error (an open
    /// end is clamped).
    pub fn resolve(&self, page_count: usize) -> Result<Vec<usize>> {
        let mut out = vec![];
        for &(a, b) in &self.ranges {
            let first = a.unwrap_or(1);
            let last = b.unwrap_or(page_count);
            if first > page_count || last > page_count {
                return Err(Error::invalid(format!(
                    "page {} is out of range (document has {page_count})",
                    first.max(last)
                )));
            }
            out.extend((first..=last).map(|p| p - 1));
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&i| match self.parity {
            Parity::All => true,
            Parity::Odd => (i + 1) % 2 == 1,
            Parity::Even => (i + 1) % 2 == 0,
        });
        if out.is_empty() {
            return Err(Error::invalid("the selection contains no pages"));
        }
        Ok(out)
    }
}

/// Zero-based inclusive ranges from user text (`"1-3,5"`), for splitting: each range stays a
/// separate part, in the order written, and may not overlap its neighbours.
pub fn parse_split_ranges(text: &str, page_count: usize) -> Result<Vec<(usize, usize)>> {
    let mut out = vec![];
    for part in text.split(',') {
        let sel = PageSelection::parse(part)?;
        let v = sel.resolve(page_count)?;
        out.push((v[0], *v.last().unwrap()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_resolves() {
        let r = |s: &str, n| PageSelection::parse(s).unwrap().resolve(n).unwrap();
        assert_eq!(r("1-3, 7, 10-", 12), [0, 1, 2, 6, 9, 10, 11]);
        assert_eq!(r("-4", 9), [0, 1, 2, 3]);
        assert_eq!(r("all", 3), [0, 1, 2]);
        assert_eq!(r("odd", 6), [0, 2, 4]);
        assert_eq!(r("even", 6), [1, 3, 5]);
        assert_eq!(r("2-9 even", 20), [1, 3, 5, 7]);
        assert_eq!(r("3,1,3", 5), [0, 2]);
        assert!(PageSelection::parse("").is_err());
        assert!(PageSelection::parse("0").is_err());
        assert!(PageSelection::parse("5-2").is_err());
        assert!(PageSelection::parse("1,,2").is_err());
        assert!(PageSelection::parse("x").is_err());
        assert!(PageSelection::parse("8").unwrap().resolve(5).is_err());
        assert!(PageSelection::parse("2 odd").unwrap().resolve(5).is_err());
    }

    #[test]
    fn split_ranges() {
        assert_eq!(
            parse_split_ranges("1-3, 4, 5-", 8).unwrap(),
            [(0, 2), (3, 3), (4, 7)]
        );
        assert!(parse_split_ranges("1-3,", 8).is_err());
    }
}
