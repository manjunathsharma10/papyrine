//! Deterministic generator for synthetic test PDFs (ARCHITECTURE section 14, ADR-017).
//!
//! Output goes to `corpus/cache/generated/` (gitignored). Run `gen-corpus --help`.

mod content;
mod doc;
mod encrypt;
mod gens;
mod malform;
mod pdf;
mod rng;

use gens::Ctx;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

type Run = Box<dyn Fn(&Ctx, &Path) -> io::Result<()>>;

struct Entry {
    name: String,
    group: &'static str,
    desc: String,
    /// Roughly this large on disk (documentation and --skip-huge only).
    approx: &'static str,
    huge: bool,
    needs_qpdf: bool,
    run: Run,
}

fn catalogue() -> Vec<Entry> {
    let mut v: Vec<Entry> = Vec::new();
    let mut add = |name: &str, group, desc: &str, approx, huge, run: Run| {
        v.push(Entry {
            name: name.into(),
            group,
            desc: desc.into(),
            approx,
            huge,
            needs_qpdf: false,
            run,
        })
    };
    add(
        "typical-20p",
        "typical",
        "20 pages, text plus one 1100x800 RGB image per page",
        "~4 MB",
        false,
        Box::new(gens::typical_20p),
    );
    add(
        "large-2000p-text",
        "large",
        "2,000 dense text and number-table pages",
        "~40 MB",
        false,
        Box::new(gens::large_2000p_text),
    );
    add(
        "large-1000p-images",
        "large",
        "1,000 full-page 2480x3508 RGB (300 dpi A4) images",
        "~450 MB",
        true,
        Box::new(gens::large_1000p_images),
    );
    add(
        "large-single-page",
        "large",
        "one 5 m x 5 m (14173 pt) page, 400k vector elements",
        "~10 MB",
        false,
        Box::new(gens::large_single_page),
    );
    add(
        "large-objects",
        "large",
        "1.6M indirect objects, classic xref table",
        "~100 MB",
        true,
        Box::new(|c, p| gens::large_objects(c, p, false)),
    );
    add(
        "large-objects-objstm",
        "large",
        "1.6M indirect objects in object streams with an xref stream",
        "~25 MB",
        false,
        Box::new(|c, p| gens::large_objects(c, p, true)),
    );
    add(
        "plain-source-5p",
        "encrypted",
        "unencrypted 5-page source (with XMP) for the encryption variants",
        "~3 KB",
        false,
        Box::new(|_, p| encrypt::plain_source(p)),
    );
    let mut out = v;
    for ev in encrypt::variants() {
        let name = ev.name;
        let desc = format!("{} [user={:?} owner={:?}]", ev.desc, ev.user, ev.owner);
        out.push(Entry {
            name: name.into(),
            group: "encrypted",
            desc,
            approx: "~3 KB",
            huge: false,
            needs_qpdf: true,
            run: Box::new(move |_, dst| {
                let src = dst.with_file_name("plain-source-5p.pdf");
                if !src.exists() {
                    encrypt::plain_source(&src)?;
                }
                let ev = encrypt::variants()
                    .into_iter()
                    .find(|e| e.name == name)
                    .unwrap();
                encrypt::encrypt(&ev, &src, dst)
            }),
        });
    }
    for (name, desc, f) in malform::catalogue() {
        out.push(Entry {
            name: name.into(),
            group: "malformed",
            desc: desc.into(),
            approx: if name.contains("bomb") {
                "~1 MB"
            } else {
                "<20 KB"
            },
            huge: false,
            needs_qpdf: false,
            run: Box::new(move |_, dst| fs::write(dst, f(&malform::bases()))),
        });
    }
    out
}

fn glob_match(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((pre, rest)) => {
            if !s.starts_with(pre) {
                return false;
            }
            let s = &s[pre.len()..];
            if rest.is_empty() {
                return true;
            }
            (0..=s.len()).any(|i| s.is_char_boundary(i) && glob_match(rest, &s[i..]))
        }
    }
}

const HELP: &str = "gen-corpus: generate deterministic synthetic test PDFs

USAGE: gen-corpus [OPTIONS]

  --out DIR        output directory (default: <repo>/corpus/cache/generated)
  --list           list every file, group and approximate size, then exit
  --only PATTERN   generate only matching names; repeatable or comma separated;
                   '*' wildcards and group names (typical, large, encrypted, malformed) work
  --skip-huge      skip the files marked huge in --list (> ~100 MB or slow)
  --force          regenerate files that already exist
  --jobs N         worker threads for page generation (default: CPU count)
  --check          run `qpdf --check` on every generated file and print a table
  --help
";

fn default_out() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap_or(Path::new("."));
    root.join("corpus/cache/generated")
}

fn qpdf_check(path: &Path) -> (i32, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut cmd = Command::new("qpdf");
    if let Some(v) = encrypt::variants().iter().find(|v| v.name == stem) {
        cmd.arg(format!("--password={}", v.user));
    }
    match cmd.arg("--check").arg(path).output() {
        Ok(o) => {
            let dir = path
                .parent()
                .map(|d| format!("{}/", d.display()))
                .unwrap_or_default();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            )
            .replace(&dir, "");
            let first = text
                .lines()
                .find(|l| {
                    l.contains("WARNING")
                        || l.contains("ERROR")
                        || l.contains("error")
                        || l.contains("password")
                })
                .unwrap_or("")
                .trim()
                .chars()
                .take(110)
                .collect::<String>();
            (o.status.code().unwrap_or(-1), first)
        }
        Err(_) => (-1, "qpdf not found".into()),
    }
}

fn human(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.2} GiB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut out, mut only, mut list, mut skip_huge, mut force, mut check) = (
        default_out(),
        Vec::<String>::new(),
        false,
        false,
        false,
        false,
    );
    let mut jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--list" => list = true,
            "--skip-huge" => skip_huge = true,
            "--force" => force = true,
            "--check" => check = true,
            "--out" => out = PathBuf::from(args.next().unwrap_or_default()),
            "--only" => only.extend(
                args.next()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::to_string),
            ),
            "--jobs" => jobs = args.next().and_then(|s| s.parse().ok()).unwrap_or(jobs),
            other => {
                eprintln!("unknown argument {other}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }
    let all = catalogue();
    let selected: Vec<&Entry> = all
        .iter()
        .filter(|e| only.is_empty() || only.iter().any(|p| p == e.group || glob_match(p, &e.name)))
        .filter(|e| !(skip_huge && e.huge))
        .collect();
    if !only.is_empty() && selected.is_empty() {
        eprintln!("no file matches --only {only:?}; see --list");
        return ExitCode::from(2);
    }
    if list {
        println!("{:<38} {:<10} {:<9} description", "name", "group", "size");
        for e in &selected {
            println!(
                "{:<38} {:<10} {:<9} {}{}",
                format!("{}.pdf", e.name),
                e.group,
                e.approx,
                e.desc,
                if e.huge { "  [huge]" } else { "" }
            );
        }
        println!("{} files", selected.len());
        return ExitCode::SUCCESS;
    }
    if let Err(e) = fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    let have_qpdf = encrypt::qpdf_available();
    let ctx = Ctx { jobs };
    let (mut ok, mut skipped, mut failed) = (0, 0, 0);
    let t_all = Instant::now();
    let mut done_paths = Vec::new();
    for e in &selected {
        let path = out.join(format!("{}.pdf", e.name));
        if path.exists() && !force {
            skipped += 1;
            done_paths.push(path);
            continue;
        }
        if e.needs_qpdf && !have_qpdf {
            eprintln!(
                "skip {}: qpdf CLI not found (needed only at generation time)",
                e.name
            );
            skipped += 1;
            continue;
        }
        let t = Instant::now();
        let tmp = out.join(format!("{}.pdf.tmp", e.name));
        let r = (e.run)(&ctx, &tmp).and_then(|_| fs::rename(&tmp, &path));
        match r {
            Ok(()) => {
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!(
                    "{:<38} {:>10} {:>7.1}s",
                    e.name,
                    human(size),
                    t.elapsed().as_secs_f64()
                );
                ok += 1;
                done_paths.push(path);
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                eprintln!("FAILED {}: {err}", e.name);
                failed += 1;
            }
        }
    }
    write_index(&out, &all);
    println!(
        "generated {ok}, already present {skipped}, failed {failed}, {:.1}s total -> {}",
        t_all.elapsed().as_secs_f64(),
        out.display()
    );
    if check {
        if !have_qpdf {
            eprintln!("--check needs the qpdf CLI");
            return ExitCode::from(2);
        }
        println!("\n{:<38} {:>3}  first message", "file", "rc");
        for p in &done_paths {
            let (rc, msg) = qpdf_check(p);
            println!(
                "{:<38} {:>3}  {}",
                p.file_name().unwrap().to_string_lossy(),
                rc,
                msg
            );
        }
        println!("(qpdf exit status: 0 clean, 3 warnings, 2 errors)");
    }
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn write_index(out: &Path, all: &[Entry]) {
    let mut idx = String::from("name\tgroup\tbytes\tdescription\n");
    let mut pw = String::from("file\tuser\towner\n");
    for e in all {
        let size = fs::metadata(out.join(format!("{}.pdf", e.name)))
            .map(|m| m.len().to_string())
            .unwrap_or_else(|_| "-".into());
        idx.push_str(&format!(
            "{}.pdf\t{}\t{}\t{}\n",
            e.name, e.group, size, e.desc
        ));
    }
    for v in encrypt::variants() {
        pw.push_str(&format!("{}.pdf\t{}\t{}\n", v.name, v.user, v.owner));
    }
    let _ = fs::write(out.join("INDEX.tsv"), idx);
    let _ = fs::write(out.join("passwords.tsv"), pw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("mal-*", "mal-prev-loop"));
        assert!(glob_match("*loop", "mal-prev-loop"));
        assert!(glob_match("a*c*e", "abcde"));
        assert!(!glob_match("mal-*", "enc-x"));
        assert!(glob_match("exact", "exact"));
    }

    #[test]
    fn names_unique() {
        let mut names: Vec<_> = catalogue().into_iter().map(|e| e.name).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(n, names.len());
        assert!(n >= 40);
    }

    #[test]
    fn malformations_are_deterministic_and_differ_from_base() {
        let b = malform::bases();
        for (name, _, f) in malform::catalogue() {
            let (x, y) = (f(&b), f(&b));
            assert_eq!(x, y, "{name} not deterministic");
            if name != "mal-incremental-update-ok-baseline" {
                assert!(
                    x != b.classic && x != b.flate && x != b.objstm && x != b.incr,
                    "{name} equals a base"
                );
            }
        }
    }
}
