//! Minimal SPDX licence-expression evaluation against the allowlist.
//!
//! `A OR B` is acceptable when either side is; `A AND B` when both are;
//! `A WITH X` when `A` is allowed and `X` is an allowed exception.

use crate::{ALLOWED, ALLOWED_EXCEPTIONS, ALLOWED_FONT_ONLY};

#[derive(Debug)]
enum Expr {
    Id(String),
    With(String, String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | ')' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Parser {
    toks: Vec<String>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(String::as_str)
    }
    fn is_kw(&self, kw: &str) -> bool {
        self.peek().is_some_and(|t| t.eq_ignore_ascii_case(kw))
    }
    fn or(&mut self) -> Result<Expr, String> {
        let mut v = vec![self.and()?];
        while self.is_kw("OR") || self.peek() == Some("/") {
            self.pos += 1;
            v.push(self.and()?);
        }
        Ok(if v.len() == 1 {
            v.pop().unwrap()
        } else {
            Expr::Or(v)
        })
    }
    fn and(&mut self) -> Result<Expr, String> {
        let mut v = vec![self.atom()?];
        while self.is_kw("AND") {
            self.pos += 1;
            v.push(self.atom()?);
        }
        Ok(if v.len() == 1 {
            v.pop().unwrap()
        } else {
            Expr::And(v)
        })
    }
    fn atom(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some("(") => {
                self.pos += 1;
                let e = self.or()?;
                if self.peek() != Some(")") {
                    return Err("missing ')'".into());
                }
                self.pos += 1;
                Ok(e)
            }
            Some(t)
                if t != ")"
                    && !["and", "or", "with"].contains(&t.to_ascii_lowercase().as_str()) =>
            {
                let id = t.trim_end_matches('+').to_string();
                self.pos += 1;
                if self.is_kw("WITH") {
                    self.pos += 1;
                    let ex = self
                        .peek()
                        .ok_or("missing exception after WITH")?
                        .to_string();
                    self.pos += 1;
                    Ok(Expr::With(id, ex))
                } else {
                    Ok(Expr::Id(id))
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }
}

fn id_allowed(id: &str, font: bool) -> Result<(), String> {
    let lower = id.to_ascii_lowercase();
    if lower.contains("gpl") {
        return Err(format!("{id} is a banned copyleft licence"));
    }
    if ALLOWED.contains(&id) || (font && ALLOWED_FONT_ONLY.contains(&id)) {
        Ok(())
    } else {
        Err(format!("{id} is not on the licence allowlist"))
    }
}

fn eval(e: &Expr, font: bool) -> Result<(), String> {
    match e {
        Expr::Id(id) => id_allowed(id, font),
        Expr::With(id, ex) => {
            id_allowed(id, font)?;
            if ALLOWED_EXCEPTIONS.contains(&ex.as_str()) {
                Ok(())
            } else {
                Err(format!("exception {ex} is not allowed"))
            }
        }
        Expr::And(v) => v.iter().try_for_each(|x| eval(x, font)),
        Expr::Or(v) => {
            let mut errs = Vec::new();
            for x in v {
                match eval(x, font) {
                    Ok(()) => return Ok(()),
                    Err(e) => errs.push(e),
                }
            }
            Err(errs.join("; "))
        }
    }
}

/// Checks an SPDX expression; `font` additionally permits OFL-1.1.
pub fn check(expr: &str, font: bool) -> Result<(), String> {
    let toks = tokenize(expr.trim());
    if toks.is_empty() {
        return Err("empty licence expression".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p
        .or()
        .map_err(|m| format!("invalid SPDX expression '{expr}': {m}"))?;
    if p.pos != p.toks.len() {
        return Err(format!("invalid SPDX expression '{expr}': trailing tokens"));
    }
    eval(&e, font)
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn accepts_dual_and_compound() {
        assert!(check("MIT OR Apache-2.0", false).is_ok());
        assert!(check("(MIT OR GPL-3.0-only) AND Zlib", false).is_ok());
        assert!(check("Apache-2.0 WITH LLVM-exception", false).is_ok());
        assert!(check("BSD-3-Clause AND IJG", false).is_ok());
    }

    #[test]
    fn rejects_copyleft_and_unknown() {
        assert!(check("GPL-3.0-only", false).is_err());
        assert!(check("MIT AND LGPL-2.1-or-later", false).is_err());
        assert!(check("AGPL-3.0", false).is_err());
        assert!(check("Proprietary", false).is_err());
        assert!(check("", false).is_err());
    }

    #[test]
    fn ofl_only_for_fonts() {
        assert!(check("OFL-1.1", false).is_err());
        assert!(check("OFL-1.1", true).is_ok());
    }
}
