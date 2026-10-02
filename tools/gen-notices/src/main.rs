//! Generates THIRD_PARTY_LICENSES.md (repo) and notices.html (shipped in the bundle).
//!
//! Sources: `cargo metadata` for the shipped graph of the app binary (normal
//! dependencies only, union over the five release targets, proc-macros and build
//! dependencies excluded), `pnpm licenses list --prod`, and `third_party/native.toml`.

use license_gate::native::Manifest;
use license_gate::repo_root;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

/// Credit lines that must appear verbatim (ARCHITECTURE section 12, ADR-014).
const REQUIRED_CREDITS: &[(&str, &str)] = &[
    (
        "Independent JPEG Group (libjpeg-turbo, mozjpeg)",
        "This software is based in part on the work of the Independent JPEG Group.",
    ),
    (
        "The FreeType Project (via PDFium)",
        "Portions of this software are copyright \u{a9} The FreeType Project (www.freetype.org). All rights reserved.",
    ),
];

#[derive(Default)]
struct Entry {
    name: String,
    version: String,
    license: String,
    texts: Vec<String>,
}

fn normalize(t: &str) -> String {
    let t = t.replace("\r\n", "\n");
    let lines: Vec<&str> = t.lines().map(str::trim_end).collect();
    lines.join("\n").trim().to_string()
}

fn read_lossy(p: &Path) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(p)?).into_owned())
}

fn license_texts_in(dir: &Path, extra: Option<&str>) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let n = e.file_name().to_string_lossy().into_owned();
            let up = n.to_ascii_uppercase();
            let stem = up.split('.').next().unwrap_or("");
            let is_lic = [
                "LICENSE",
                "LICENCE",
                "COPYING",
                "NOTICE",
                "UNLICENSE",
                "COPYRIGHT",
            ]
            .iter()
            .any(|p| stem.starts_with(p));
            let ext_ok = !up.ends_with(".RS") && !up.ends_with(".TOML") && !up.ends_with(".PY");
            if is_lic && ext_ok && e.path().is_file() {
                names.insert(n);
            }
        }
    }
    if let Some(x) = extra {
        names.insert(x.to_string());
    }
    names
        .iter()
        .filter_map(|n| read_lossy(&dir.join(n)).ok())
        .map(|t| normalize(&t))
        .filter(|t| !t.is_empty())
        .collect()
}

fn cargo_metadata(root: &Path, target: &str) -> Result<Value, String> {
    let out = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            target,
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

fn rust_entries(root: &Path) -> Result<Vec<Entry>, String> {
    let mut found: BTreeMap<(String, String), Entry> = BTreeMap::new();
    for target in TARGETS {
        let md = cargo_metadata(root, target)?;
        let pkgs: BTreeMap<&str, &Value> = md["packages"]
            .as_array()
            .ok_or("no packages")?
            .iter()
            .map(|p| (p["id"].as_str().unwrap_or(""), p))
            .collect();
        let members: BTreeSet<&str> = md["workspace_members"]
            .as_array()
            .ok_or("no members")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let roots: Vec<&str> = members
            .iter()
            .copied()
            .filter(|id| {
                let p = pkgs[id];
                let path = p["manifest_path"].as_str().unwrap_or("").replace('\\', "/");
                p["name"] == "papyrine-app" || path.contains("/apps/")
            })
            .collect();
        let nodes: BTreeMap<&str, &Value> = md["resolve"]["nodes"]
            .as_array()
            .ok_or("no resolve")?
            .iter()
            .map(|n| (n["id"].as_str().unwrap_or(""), n))
            .collect();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack = roots.clone();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(node) = nodes.get(id) else { continue };
            let is_proc_macro = pkgs[id]["targets"].as_array().is_some_and(|ts| {
                ts.iter().any(|t| {
                    t["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|x| x == "proc-macro"))
                })
            });
            if is_proc_macro {
                continue;
            }
            for d in node["deps"].as_array().into_iter().flatten() {
                let normal = d["dep_kinds"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|x| x["kind"].is_null()));
                if normal && let Some(p) = d["pkg"].as_str() {
                    stack.push(p);
                }
            }
        }
        for id in seen {
            let p = pkgs[id];
            let is_proc_macro = p["targets"].as_array().is_some_and(|ts| {
                ts.iter().any(|t| {
                    t["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|x| x == "proc-macro"))
                })
            });
            if p["source"].is_null() || is_proc_macro {
                continue; // first-party or not linked into the binary
            }
            let name = p["name"].as_str().unwrap_or("").to_string();
            let version = p["version"].as_str().unwrap_or("").to_string();
            found
                .entry((name.clone(), version.clone()))
                .or_insert_with(|| {
                    let dir = Path::new(p["manifest_path"].as_str().unwrap_or(""))
                        .parent()
                        .unwrap()
                        .to_path_buf();
                    let lic_file = p["license_file"].as_str();
                    Entry {
                        name,
                        version,
                        license: p["license"]
                            .as_str()
                            .unwrap_or("(see license file)")
                            .to_string(),
                        texts: license_texts_in(&dir, lic_file),
                    }
                });
        }
    }
    Ok(found.into_values().collect())
}

fn js_entries(root: &Path) -> Result<Vec<Entry>, String> {
    let dir = root.join("apps/desktop");
    if !dir.join("package.json").exists() {
        return Ok(Vec::new());
    }
    if !dir.join("node_modules").exists() {
        return Err("apps/desktop/node_modules missing: run `pnpm install`".into());
    }
    let out = Command::new(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" })
        .args(["licenses", "list", "--prod", "--json"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("cannot run pnpm: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pnpm licenses failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut map: BTreeMap<(String, String), Entry> = BTreeMap::new();
    for (license, pkgs) in v.as_object().into_iter().flatten() {
        for p in pkgs.as_array().into_iter().flatten() {
            let name = p["name"].as_str().unwrap_or("").to_string();
            let versions: Vec<&str> = p["versions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let paths: Vec<&str> = p["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for (i, ver) in versions.iter().enumerate() {
                let texts = paths
                    .get(i)
                    .or(paths.first())
                    .map(|p| license_texts_in(Path::new(p), None))
                    .unwrap_or_default();
                map.insert(
                    (name.clone(), ver.to_string()),
                    Entry {
                        name: name.clone(),
                        version: ver.to_string(),
                        license: license.clone(),
                        texts,
                    },
                );
            }
        }
    }
    Ok(map.into_values().collect())
}

/// Native entries plus any missing-text problems.
fn native_entries(root: &Path, allow_missing: bool) -> Result<(Vec<Entry>, Vec<String>), String> {
    let path = root.join("third_party/native.toml");
    if !path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let m = Manifest::load(&path)?;
    let mut out = Vec::new();
    let mut credits = Vec::new();
    for lib in &m.library {
        let mut texts = Vec::new();
        for f in &lib.license_files {
            match lib.resolve_file(root, f) {
                Some(p) => {
                    let t = read_lossy(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                    texts.push(normalize(&t));
                }
                None if allow_missing => {}
                None => {
                    return Err(format!(
                        "native library '{}': licence file '{f}' not found; run third_party/fetch first",
                        lib.name
                    ));
                }
            }
        }
        if let Some(c) = &lib.credit {
            credits.push(c.clone());
        }
        out.push(Entry {
            name: lib.name.clone(),
            version: lib.version.clone(),
            license: lib.spdx.clone(),
            texts,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((out, credits))
}

/// Standard licence bodies that are factored out of the per-crate texts (they repeat verbatim).
/// (name, first words, last words, tail is the last occurrence)
const STANDARD: &[(&str, &[&str], &[&str], bool)] = &[
    (
        "Apache License 2.0",
        &["Apache", "License", "Version", "2.0,"],
        &["END", "OF", "TERMS", "AND", "CONDITIONS"],
        false,
    ),
    (
        "MIT License",
        &[
            "Permission",
            "is",
            "hereby",
            "granted,",
            "free",
            "of",
            "charge,",
        ],
        &["DEALINGS", "IN", "THE", "SOFTWARE."],
        false,
    ),
    (
        "Mozilla Public License 2.0",
        &["Mozilla", "Public", "License", "Version", "2.0"],
        &["Mozilla", "Public", "License,", "v.", "2.0."],
        true,
    ),
];

fn words(t: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut start = None;
    for (i, c) in t.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(st)) => {
                v.push((st, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(st) = start {
        v.push((st, t.len()));
    }
    v
}

/// URL scheme and trailing-path variants of the same licence link compare equal.
fn canon_word(w: &str) -> String {
    if w.contains("apache.org/licenses") {
        "<apache-url>".into()
    } else {
        w.to_string()
    }
}

type Body = (usize, usize, Vec<String>);

/// Byte range of a standard body in `t`, with its canonical word list.
fn find_body(t: &str, head: &[&str], tail: &[&str], last: bool) -> Option<Body> {
    let w = words(t);
    let tok = |i: usize| &t[w[i].0..w[i].1];
    let matches = |i: usize, pat: &[&str]| {
        i + pat.len() <= w.len() && pat.iter().enumerate().all(|(k, p)| tok(i + k) == *p)
    };
    let a = (0..w.len()).find(|&i| matches(i, head))?;
    let b = if last {
        (a..w.len()).rev().find(|&i| matches(i, tail))?
    } else {
        (a..w.len()).find(|&i| matches(i, tail))?
    } + tail.len()
        - 1;
    Some((
        w[a].0,
        w[b].1,
        (a..=b).map(|i| canon_word(tok(i))).collect(),
    ))
}

/// Replaces the most common standard body of each kind by a placeholder; returns those bodies.
fn factor_standard(lists: &mut [&mut Vec<Entry>]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (name, head, tail, last) in STANDARD {
        let mut counts: BTreeMap<Vec<String>, (usize, String)> = BTreeMap::new();
        for e in lists.iter().flat_map(|l| l.iter()) {
            for t in &e.texts {
                if let Some((a, b, ws)) = find_body(t, head, tail, *last) {
                    counts.entry(ws).or_insert((0, t[a..b].to_string())).0 += 1;
                }
            }
        }
        let Some((canon_ws, (n, body))) = counts.into_iter().max_by_key(|(_, (n, _))| *n) else {
            continue;
        };
        if n < 2 {
            continue;
        }
        for e in lists.iter_mut().flat_map(|l| l.iter_mut()) {
            for t in e.texts.iter_mut() {
                if let Some((a, b, ws)) = find_body(t, head, tail, *last)
                    && ws == canon_ws
                {
                    *t = format!(
                        "{}[{name}: standard text, see \"Standard licence texts\" below]{}",
                        &t[..a],
                        &t[b..]
                    );
                }
            }
        }
        out.push((*name, body));
    }
    out
}

enum Block {
    H(usize, String),
    P(String),
    Table(Vec<[String; 3]>),
    Text(String, String),
}

fn grouped_texts(entries: &[Entry]) -> Vec<(String, Vec<String>)> {
    let mut by_text: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for e in entries {
        for t in &e.texts {
            by_text
                .entry(t)
                .or_default()
                .push(format!("{} {}", e.name, e.version));
        }
    }
    let mut v: Vec<(String, Vec<String>)> = by_text
        .into_iter()
        .map(|(t, mut u)| {
            u.sort();
            u.dedup();
            (t.to_string(), u)
        })
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

fn table(entries: &[Entry]) -> Block {
    Block::Table(
        entries
            .iter()
            .map(|e| [e.name.clone(), e.version.clone(), e.license.clone()])
            .collect(),
    )
}

fn build(root: &Path, allow_missing: bool) -> Result<Vec<Block>, String> {
    let mut rust = rust_entries(root)?;
    let mut js = js_entries(root)?;
    let (mut native, mut credits) = native_entries(root, allow_missing)?;
    let standard = factor_standard(&mut [&mut native, &mut rust, &mut js]);
    let mut b = vec![
        Block::H(1, "Third-party licenses".into()),
        Block::P("Papyrine is licensed under MIT OR Apache-2.0. It includes the third-party software below. This file is generated by `tools/gen-notices`; do not edit it by hand.".into()),
        Block::H(2, "Required attributions".into()),
    ];
    for (who, line) in REQUIRED_CREDITS {
        b.push(Block::P(format!("{who}: {line}")));
    }
    credits.sort();
    credits.dedup();
    for c in credits {
        b.push(Block::P(c));
    }
    b.push(Block::H(2, "Native libraries".into()));
    b.push(table(&native));
    let mut seen: BTreeMap<&str, String> = BTreeMap::new();
    for e in &native {
        b.push(Block::H(
            3,
            format!("{} {} ({})", e.name, e.version, e.license),
        ));
        for t in &e.texts {
            match seen.get(t.as_str()) {
                Some(first) => b.push(Block::P(format!("Same licence text as {first}."))),
                None => {
                    seen.insert(t, e.name.clone());
                    b.push(Block::Text(String::new(), t.clone()));
                }
            }
        }
    }
    b.push(Block::H(2, format!("Rust crates ({})", rust.len())));
    b.push(table(&rust));
    b.push(Block::H(3, "Rust licence texts".into()));
    for (t, users) in grouped_texts(&rust) {
        b.push(Block::Text(users.join(", "), t));
    }
    b.push(Block::H(2, format!("JavaScript packages ({})", js.len())));
    b.push(table(&js));
    b.push(Block::H(3, "JavaScript licence texts".into()));
    for (t, users) in grouped_texts(&js) {
        b.push(Block::Text(users.join(", "), t));
    }
    b.push(Block::H(2, "Standard licence texts".into()));
    for (name, body) in standard {
        b.push(Block::Text(name.to_string(), body));
    }
    Ok(b)
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn fence_for(t: &str) -> String {
    let mut n = 3;
    while t.contains(&"`".repeat(n)) {
        n += 1;
    }
    "`".repeat(n)
}

fn render_md(blocks: &[Block]) -> String {
    let mut s = String::new();
    for b in blocks {
        match b {
            Block::H(n, t) => s.push_str(&format!("{} {t}\n\n", "#".repeat(*n))),
            Block::P(t) => s.push_str(&format!("{t}\n\n")),
            Block::Table(rows) => {
                if rows.is_empty() {
                    s.push_str("None.\n\n");
                    continue;
                }
                s.push_str("| Name | Version | License |\n|---|---|---|\n");
                for r in rows {
                    s.push_str(&format!(
                        "| {} | {} | {} |\n",
                        md_cell(&r[0]),
                        md_cell(&r[1]),
                        md_cell(&r[2])
                    ));
                }
                s.push('\n');
            }
            Block::Text(users, t) => {
                if !users.is_empty() {
                    s.push_str(&format!("Used by: {users}\n\n"));
                }
                let f = fence_for(t);
                s.push_str(&format!("{f}text\n{t}\n{f}\n\n"));
            }
        }
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_html(blocks: &[Block]) -> String {
    let mut s = String::from(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Third-party licenses</title><style>body{font:14px/1.5 system-ui,sans-serif;margin:2rem auto;max-width:60rem;padding:0 1rem}pre{white-space:pre-wrap;background:#8881;padding:.75rem;border-radius:6px}td,th{text-align:left;padding:2px 12px 2px 0}</style></head><body>\n",
    );
    for b in blocks {
        match b {
            Block::H(n, t) => s.push_str(&format!("<h{n}>{}</h{n}>\n", esc(t))),
            Block::P(t) => s.push_str(&format!("<p>{}</p>\n", esc(t))),
            Block::Table(rows) => {
                s.push_str("<table><tr><th>Name</th><th>Version</th><th>License</th></tr>\n");
                for r in rows {
                    s.push_str(&format!(
                        "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                        esc(&r[0]),
                        esc(&r[1]),
                        esc(&r[2])
                    ));
                }
                s.push_str("</table>\n");
            }
            Block::Text(users, t) => {
                if !users.is_empty() {
                    s.push_str(&format!("<p>Used by: {}</p>\n", esc(users)));
                }
                s.push_str(&format!("<pre>{}</pre>\n", esc(t)));
            }
        }
    }
    s.push_str("</body></html>\n");
    s
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!(
            "usage: gen-notices [--check] [--out THIRD_PARTY_LICENSES.md] [--html notices.html] [--allow-missing]"
        );
        return ExitCode::SUCCESS;
    }
    let root = arg(&args, "--root")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root(&std::env::current_dir().unwrap()));
    let out = root.join(arg(&args, "--out").unwrap_or_else(|| "THIRD_PARTY_LICENSES.md".into()));
    let check = args.iter().any(|a| a == "--check");
    let blocks = match build(&root, args.iter().any(|a| a == "--allow-missing")) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("gen-notices: FAIL: {e}");
            return ExitCode::from(1);
        }
    };
    let md = render_md(&blocks);
    if let Some(h) = arg(&args, "--html")
        && let Err(e) = std::fs::write(&h, render_html(&blocks))
    {
        eprintln!("gen-notices: cannot write {h}: {e}");
        return ExitCode::from(1);
    }
    if check {
        let have = std::fs::read_to_string(&out)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if have == md {
            println!("gen-notices: PASS ({} is up to date)", out.display());
            return ExitCode::SUCCESS;
        }
        let line = have
            .lines()
            .zip(md.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(have.lines().count().min(md.lines().count()));
        if std::env::var_os("GITHUB_ACTIONS").is_some() {
            println!("::error title=gen-notices::THIRD_PARTY_LICENSES.md is stale");
        }
        eprintln!(
            "gen-notices: FAIL: {} is stale (first difference at line {}). Run `cargo run -p gen-notices` and commit the result.",
            out.display(),
            line + 1
        );
        return ExitCode::from(1);
    }
    if let Err(e) = std::fs::write(&out, md) {
        eprintln!("gen-notices: cannot write {}: {e}", out.display());
        return ExitCode::from(1);
    }
    println!("gen-notices: wrote {}", out.display());
    ExitCode::SUCCESS
}
