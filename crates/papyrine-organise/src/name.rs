//! File names for the parts of a split, from a template.
//!
//! Placeholders: `{name}` (source file stem), `{n}` (part number from 1; `{n:03}` pads to 3
//! digits), `{first}` and `{last}` (one-based source pages), `{pages}` (`3-7`, or `3` for one
//! page), `{title}` (the part's first top-level bookmark, or the file stem). `{{` and `}}` are
//! literal braces.

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct NameVars {
    pub name: String,
    /// One-based part number.
    pub n: usize,
    /// One-based first and last source page of the part.
    pub first: usize,
    pub last: usize,
    pub title: Option<String>,
}

/// Expand `template` (without extension) and make the result a safe file name stem.
pub fn render_name(template: &str, v: &NameVars) -> Result<String> {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut key = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => key.push(ch),
                        None => return Err(Error::invalid("unclosed `{` in name template")),
                    }
                }
                let (key, width) = match key.split_once(':') {
                    Some((k, w)) => (
                        k,
                        w.parse::<usize>().map_err(|_| {
                            Error::invalid(format!("bad width `{w}` in name template"))
                        })?,
                    ),
                    None => (key.as_str(), 0),
                };
                let num = |x: usize| format!("{x:0width$}");
                match key {
                    "name" => out.push_str(&v.name),
                    "n" => out.push_str(&num(v.n)),
                    "first" => out.push_str(&num(v.first)),
                    "last" => out.push_str(&num(v.last)),
                    "pages" if v.first == v.last => out.push_str(&num(v.first)),
                    "pages" => out.push_str(&format!("{}-{}", num(v.first), num(v.last))),
                    "title" => out.push_str(v.title.as_deref().unwrap_or(&v.name)),
                    other => {
                        return Err(Error::invalid(format!(
                            "unknown placeholder `{{{other}}}` in name template"
                        )));
                    }
                }
            }
            '}' => return Err(Error::invalid("unmatched `}` in name template")),
            c => out.push(c),
        }
    }
    Ok(sanitize_file_name(&out))
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A file name stem that is valid on Windows, macOS and Linux: no separators or control
/// characters, no trailing dots or spaces, no reserved device names, at most 100 characters.
pub fn sanitize_file_name(s: &str) -> String {
    let mut t: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    t = t.trim().trim_end_matches(['.', ' ']).to_string();
    if t.chars().count() > 100 {
        t = t
            .chars()
            .take(100)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    if t.is_empty() {
        t = "part".into();
    }
    let stem = t.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        t.insert(0, '_');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> NameVars {
        NameVars {
            name: "Report".into(),
            n: 3,
            first: 7,
            last: 12,
            title: Some("Chapter 2: Methods".into()),
        }
    }

    #[test]
    fn templates() {
        let v = vars();
        assert_eq!(render_name("{name}_{n:03}", &v).unwrap(), "Report_003");
        assert_eq!(render_name("{name}_{pages}", &v).unwrap(), "Report_7-12");
        assert_eq!(
            render_name("{title} ({first}-{last})", &v).unwrap(),
            "Chapter 2_ Methods (7-12)"
        );
        assert_eq!(render_name("{{{n}}}", &v).unwrap(), "{3}");
        let one = NameVars { last: 7, ..vars() };
        assert_eq!(render_name("p{pages:3}", &one).unwrap(), "p007");
        assert!(render_name("{nope}", &v).is_err());
        assert!(render_name("{n", &v).is_err());
        assert!(render_name("a}", &v).is_err());
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize_file_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_file_name("name. "), "name");
        assert_eq!(sanitize_file_name(""), "part");
        assert_eq!(sanitize_file_name("con"), "_con");
        assert_eq!(sanitize_file_name("NUL.txt"), "_NUL.txt");
        assert_eq!(sanitize_file_name(&"x".repeat(300)).len(), 100);
    }
}
