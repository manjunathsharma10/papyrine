//! Classify scripts read as JSON lines.
//!
//! Input (stdin or the file named by the first argument): one JSON object per
//! line with at least a string field `script`; every other string field is
//! echoed back. Output: one JSON object per line with `verdict`
//! (`empty` | `accepted` | `boilerplate` | `rejected`), `detail`, `blocker`
//! and `fingerprint` (hex, of the normalised token stream; useful for adding
//! boilerplate entries).
//!
//! `classify --fingerprints` prints only `fingerprint<TAB>script-prefix` pairs.

use std::io::{BufRead, Write};

use papyrine_forms::boilerplate::fingerprint;
use papyrine_forms::{DateEnv, Event, Verdict, analyze, run_script};

struct Json<'a> {
    b: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out: Vec<u16> = Vec::new();
        let mut buf = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch: u16 = match e {
                        b'n' => 10,
                        b't' => 9,
                        b'r' => 13,
                        b'b' => 8,
                        b'f' => 12,
                        b'/' | b'\\' | b'"' => u16::from(e),
                        b'u' => {
                            let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            u16::from_str_radix(h, 16).ok()?
                        }
                        _ => return None,
                    };
                    out.push(ch);
                }
                _ => {
                    buf.clear();
                    buf.push(c);
                    let extra = match c {
                        0xC0..=0xDF => 1,
                        0xE0..=0xEF => 2,
                        0xF0..=0xF7 => 3,
                        _ => 0,
                    };
                    for _ in 0..extra {
                        buf.push(*self.b.get(self.i)?);
                        self.i += 1;
                    }
                    out.extend(String::from_utf8_lossy(&buf).encode_utf16());
                }
            }
        }
        Some(String::from_utf16_lossy(&out))
    }
    /// Skip any JSON value (strings handled by `string`).
    fn skip(&mut self) -> Option<()> {
        self.ws();
        match self.b.get(self.i)? {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let mut depth = 0;
                while self.i < self.b.len() {
                    match self.b[self.i] {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.i += 1;
                                return Some(());
                            }
                        }
                        _ => {}
                    }
                    self.i += 1;
                }
                return None;
            }
            _ => {
                while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}') {
                    self.i += 1;
                }
            }
        }
        Some(())
    }
    fn object(&mut self) -> Option<Vec<(String, String)>> {
        self.ws();
        if self.b.get(self.i) != Some(&b'{') {
            return None;
        }
        self.i += 1;
        let mut kv = Vec::new();
        loop {
            self.ws();
            match self.b.get(self.i)? {
                b'}' => {
                    self.i += 1;
                    return Some(kv);
                }
                b',' => self.i += 1,
                _ => {
                    let k = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    self.ws();
                    if self.b.get(self.i) == Some(&b'"') {
                        kv.push((k, self.string()?));
                    } else {
                        self.skip()?;
                    }
                }
            }
        }
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fp_mode = args.iter().any(|a| a == "--fingerprints");
    let path = args.iter().find(|a| !a.starts_with("--"));
    let input: Box<dyn BufRead> = match path {
        Some(p) => Box::new(std::io::BufReader::new(
            std::fs::File::open(p).expect("open input"),
        )),
        None => Box::new(std::io::BufReader::new(std::io::stdin())),
    };
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(kv) = (Json {
            b: line.as_bytes(),
            i: 0,
        })
        .object() else {
            let _ = writeln!(
                out,
                "{{\"verdict\":\"error\",\"detail\":\"bad json line\"}}"
            );
            continue;
        };
        let Some(script) = kv
            .iter()
            .find(|(k, _)| k == "script")
            .map(|(_, v)| v.clone())
        else {
            continue;
        };
        let fp = fingerprint(&script).map(|h| format!("{h:016x}"));
        if fp_mode {
            let head: String = script
                .chars()
                .take(60)
                .collect::<String>()
                .replace('\n', " ");
            let _ = writeln!(out, "{}\t{head}", fp.as_deref().unwrap_or("-"));
            continue;
        }
        let mut exec = String::new();
        let (verdict, detail, blocker) = match analyze(&script) {
            Verdict::Empty => ("empty", String::new(), String::new()),
            Verdict::Accepted(c) => {
                // Smoke-run accepted scripts in every event mode: nothing may throw or panic.
                let none = std::collections::HashMap::<String, String>::new();
                let env = DateEnv::default();
                let ok = ["1234.5", "", "03/07/2023", "-5", "abc"].iter().all(|v| {
                    [
                        Event::format(v),
                        Event::commit(v),
                        Event::keystroke(v, "1", 0, 0),
                    ]
                    .into_iter()
                    .all(|mut ev| {
                        !matches!(
                            run_script(&script, &mut ev, &none, &env),
                            Err(papyrine_forms::RunError::Rejected(_))
                        )
                    })
                });
                exec = if ok { "ok" } else { "error" }.to_string();
                ("accepted", format!("{} calls", c.len()), String::new())
            }
            Verdict::Boilerplate(n) => ("boilerplate", n.to_string(), String::new()),
            Verdict::Rejected { reason, blocker } => {
                ("rejected", reason.to_string(), blocker.label().to_string())
            }
        };
        let mut o = String::from("{");
        for (k, v) in kv.iter().filter(|(k, _)| k != "script") {
            o.push_str(&format!("{}:{},", esc(k), esc(v)));
        }
        o.push_str(&format!(
            "\"verdict\":{},\"detail\":{},\"blocker\":{},\"exec\":{},\"fingerprint\":{}}}",
            esc(verdict),
            esc(&detail),
            esc(&blocker),
            esc(&exec),
            esc(fp.as_deref().unwrap_or(""))
        ));
        let _ = writeln!(out, "{o}");
    }
}
