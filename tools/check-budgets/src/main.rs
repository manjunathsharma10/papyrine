//! Budget gates for ARCHITECTURE section 1.1.
//!
//! check-budgets [--platform macos|linux|windows] [--reference]
//!   [--installer FILE]...  [--js-dist DIR]
//!   [--launch launch.json]  [--memory memory.json]  [--large-doc large.json]
//!   [--startup-trace trace.json] [--startup-allowlist FILE]
//!   [--baseline sizes.json] [--labels a,b,c] [--history history.json]
//!   [--write-sizes sizes.json]
//!
//! Input formats (all JSON, produced by measurement scripts):
//!   launch.json  {"cold_ms": 850, "warm_ms": 400}
//!   memory.json  {"idle_mb": 140}        (sum over all app processes)
//!   large.json   [{"file":"2000p.pdf","peak_mb":380,"settled_mb":200,"engine_mb":100,"renderer_mb":90}]
//!   trace.json   {"spans":[{"name":"subsystem.window","t_ms":3.1}],"first_paint_ms":120}
//!   history.json {"cold_ms":[...],"idle_mb":[...]}   (rolling samples from earlier main runs)
//!   sizes.json   {"artifacts":{"dmg":123,...},"js_initial_gzip":456}  (baseline from main)
//! Sizes use binary units (1 MB = 1,048,576 bytes; 1 KB = 1,024 bytes).

use flate2::{Compression, write::GzEncoder};
use license_gate::{Report, glob_match, repo_root};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MB: u64 = 1024 * 1024;
const KB: u64 = 1024;
const OVERRIDE_LABEL: &str = "size-increase-approved";
const GROWTH_LIMIT: f64 = 0.05;
const TREND_LIMIT: f64 = 0.15;

#[derive(Clone, Copy, PartialEq)]
enum Platform {
    Macos,
    Linux,
    Windows,
}

fn kind_of(name: &str) -> Option<(&'static str, u64)> {
    let n = name.to_ascii_lowercase();
    let k = [
        (".appimage", "appimage", 100 * MB),
        (".dmg", "dmg", 50 * MB),
        (".msi", "msi", 50 * MB),
        (".exe", "exe", 50 * MB),
        (".deb", "deb", 50 * MB),
        (".rpm", "rpm", 50 * MB),
        (".flatpak", "flatpak", 50 * MB),
    ];
    k.iter()
        .find(|(ext, _, _)| n.ends_with(ext))
        .map(|(_, k, l)| (*k, *l))
}

fn read_json(path: &str, report: &mut Report) -> Option<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                report.fail(format!("{path}: invalid JSON: {e}"));
                None
            }
        },
        Err(e) => {
            report.fail(format!("{path}: {e}"));
            None
        }
    }
}

fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let m = s.len() / 2;
    Some(if s.len() % 2 == 1 {
        s[m]
    } else {
        (s[m - 1] + s[m]) / 2.0
    })
}

fn gzip_len(bytes: &[u8]) -> u64 {
    let mut e = GzEncoder::new(Vec::new(), Compression::best());
    e.write_all(bytes).unwrap();
    e.finish().unwrap().len() as u64
}

/// Script files referenced by index.html (script src, modulepreload): the initial bundle.
fn initial_scripts(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let is_script = tag.starts_with("script");
        let is_preload = tag.starts_with("link") && tag.contains("modulepreload");
        if is_script || is_preload {
            for attr in ["src=", "href="] {
                if let Some(p) = tag.find(attr) {
                    let v = &tag[p + attr.len()..];
                    let q = v.chars().next().unwrap_or('"');
                    if let Some(e) = v[1..].find(q) {
                        let url = &v[1..1 + e];
                        if url.ends_with(".js") || url.ends_with(".mjs") {
                            out.push(
                                url.trim_start_matches('/')
                                    .trim_start_matches("./")
                                    .to_string(),
                            );
                        }
                    }
                }
            }
        }
        rest = &rest[end..];
    }
    out
}

fn walk_js(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            walk_js(&p, out);
        } else if p.extension().is_some_and(|x| x == "js" || x == "mjs") {
            out.push(p);
        }
    }
}

struct Ctx {
    platform: Platform,
    reference: bool,
    labels: Vec<String>,
}

fn check_installers(
    ctx: &Ctx,
    files: &[String],
    baseline: Option<&Value>,
    sizes: &mut BTreeMap<String, u64>,
    report: &mut Report,
) {
    for f in files {
        let name = Path::new(f)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let Some((kind, limit)) = kind_of(&name) else {
            report.note(format!("{name}: not an installer type; ignored"));
            continue;
        };
        let size = match std::fs::metadata(f) {
            Ok(m) => m.len(),
            Err(e) => {
                report.fail(format!("{f}: {e}"));
                continue;
            }
        };
        sizes.insert(kind.to_string(), size);
        report.note(format!(
            "{name}: {:.1} MB (budget {} MB)",
            size as f64 / MB as f64,
            limit / MB
        ));
        if size > limit {
            report.fail(format!(
                "installer {name} is {:.1} MB, over the {} MB budget",
                size as f64 / MB as f64,
                limit / MB
            ));
        } else if size > 30 * MB && limit == 50 * MB {
            report.note(format!("WARN {name} exceeds the 30 MB internal target"));
        }
        if let Some(base) = baseline.and_then(|b| b["artifacts"][kind].as_u64()) {
            growth(&name, size, base, ctx, report);
        }
    }
}

fn growth(what: &str, now: u64, base: u64, ctx: &Ctx, report: &mut Report) {
    if base == 0 {
        return;
    }
    let g = now as f64 / base as f64 - 1.0;
    if g > GROWTH_LIMIT {
        if ctx.labels.iter().any(|l| l == OVERRIDE_LABEL) {
            report.note(format!(
                "{what} grew {:.1}% vs main; allowed by label {OVERRIDE_LABEL}",
                g * 100.0
            ));
        } else {
            report.fail(format!(
                "{what} grew {:.1}% vs main ({base} -> {now} bytes), over the 5% limit; add the '{OVERRIDE_LABEL}' label if intended",
                g * 100.0
            ));
        }
    }
}

fn check_js(
    ctx: &Ctx,
    dist: &str,
    baseline: Option<&Value>,
    sizes: &mut BTreeMap<String, u64>,
    report: &mut Report,
) {
    let dist = Path::new(dist);
    let html = match std::fs::read_to_string(dist.join("index.html")) {
        Ok(h) => h,
        Err(e) => {
            report.fail(format!("{}/index.html: {e}", dist.display()));
            return;
        }
    };
    let initial: Vec<String> = initial_scripts(&html);
    let mut all = Vec::new();
    walk_js(dist, &mut all);
    let mut initial_total = 0;
    for rel in &initial {
        match std::fs::read(dist.join(rel)) {
            Ok(b) => initial_total += gzip_len(&b),
            Err(e) => report.fail(format!("initial script {rel}: {e}")),
        }
    }
    for p in &all {
        let rel = p
            .strip_prefix(dist)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if initial.contains(&rel) {
            continue;
        }
        let g = gzip_len(&std::fs::read(p).unwrap_or_default());
        if g > 150 * KB {
            report.note(format!(
                "WARN lazy chunk {rel} is {:.1} KB gzipped (> 150 KB)",
                g as f64 / KB as f64
            ));
        }
    }
    sizes.insert("js_initial_gzip".into(), initial_total);
    report.note(format!(
        "initial JS: {} file(s), {:.1} KB gzipped (budget 200 KB)",
        initial.len(),
        initial_total as f64 / KB as f64
    ));
    if initial.is_empty() {
        report.fail("index.html references no initial script; cannot measure the initial bundle");
    }
    if initial_total > 200 * KB {
        report.fail(format!(
            "initial JS bundle is {:.1} KB gzipped, over the 200 KB budget",
            initial_total as f64 / KB as f64
        ));
    }
    if let Some(base) = baseline.and_then(|b| b["js_initial_gzip"].as_u64()) {
        growth("initial JS bundle", initial_total, base, ctx, report);
    }
}

fn trend(label: &str, now: f64, history: Option<&Value>, key: &str, report: &mut Report) {
    let samples: Vec<f64> = history
        .and_then(|h| h[key].as_array())
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    if let Some(m) = median(&samples)
        && m > 0.0
    {
        let g = now / m - 1.0;
        report.note(format!(
            "{label}: {now:.0} vs rolling median {m:.0} ({:+.1}%)",
            g * 100.0
        ));
        if g > TREND_LIMIT {
            report.fail(format!(
                "{label} regressed {:.1}% vs the rolling median ({m:.0} -> {now:.0}), limit 15%",
                g * 100.0
            ));
        }
    }
}

fn check_launch(ctx: &Ctx, v: &Value, history: Option<&Value>, report: &mut Report) {
    let cold = v["cold_ms"].as_f64();
    let warm = v["warm_ms"].as_f64();
    if cold.is_none() {
        report.fail("launch.json has no cold_ms");
    }
    if let Some(c) = cold {
        let limit = if ctx.reference { 1000.0 } else { 2000.0 };
        report.note(format!("cold launch {c:.0} ms (limit {limit:.0} ms)"));
        if c > limit {
            report.fail(format!("cold launch {c:.0} ms exceeds {limit:.0} ms"));
        }
        trend("cold launch", c, history, "cold_ms", report);
    }
    if ctx.reference
        && let Some(w) = warm
    {
        report.note(format!("warm launch {w:.0} ms (limit 500 ms)"));
        if w > 500.0 {
            report.fail(format!("warm launch {w:.0} ms exceeds 500 ms"));
        }
    }
}

fn check_memory(ctx: &Ctx, v: &Value, history: Option<&Value>, report: &mut Report) {
    let Some(idle) = v["idle_mb"].as_f64() else {
        report.fail("memory.json has no idle_mb");
        return;
    };
    let limit = match ctx.platform {
        Platform::Macos => 150.0,
        Platform::Linux => 175.0,
        Platform::Windows => 200.0,
    };
    report.note(format!("idle memory {idle:.0} MB (budget {limit:.0} MB)"));
    if ctx.platform == Platform::Windows {
        if idle > limit {
            report.note(format!(
                "WARN idle memory {idle:.0} MB over {limit:.0} MB; Windows is trend-gated"
            ));
        }
        trend("idle memory", idle, history, "idle_mb", report);
    } else if idle > limit {
        report.fail(format!("idle memory {idle:.0} MB exceeds {limit:.0} MB"));
    }
}

fn check_large(ctx: &Ctx, v: &Value, report: &mut Report) {
    let rows = v.as_array().or_else(|| v["files"].as_array());
    let Some(rows) = rows else {
        report.fail("large-doc JSON must be an array of {file, peak_mb, settled_mb, engine_mb, renderer_mb}");
        return;
    };
    let limits = [
        ("peak_mb", 400.0),
        ("settled_mb", 250.0),
        ("engine_mb", 120.0),
        ("renderer_mb", 120.0),
    ];
    for r in rows {
        let file = r["file"].as_str().unwrap_or("?");
        for (k, lim) in limits {
            let Some(x) = r[k].as_f64() else {
                report.fail(format!("large-doc {file}: missing {k}"));
                continue;
            };
            if x > lim {
                let msg = format!("large-doc {file}: {k} = {x:.0} MB exceeds {lim:.0} MB");
                if ctx.platform == Platform::Windows {
                    report.note(format!("WARN {msg} (Windows is trend-gated)"));
                } else {
                    report.fail(msg);
                }
            }
        }
    }
    report.note(format!("{} large-document row(s) checked", rows.len()));
}

fn allowed(sub: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|a| {
        let a = a.strip_prefix("subsystem.").unwrap_or(a);
        match a.strip_suffix(".*") {
            Some(family) => sub == family || sub.starts_with(&format!("{family}.")),
            None => glob_match(a, sub),
        }
    })
}

fn check_startup(trace: &Value, allowlist: &str, report: &mut Report) {
    let patterns: Vec<String> = match std::fs::read_to_string(allowlist) {
        Ok(s) => s
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        Err(e) => {
            report.fail(format!("{allowlist}: {e}"));
            return;
        }
    };
    let Some(fp) = trace["first_paint_ms"].as_f64() else {
        report.fail("startup trace has no first_paint_ms");
        return;
    };
    let mut bad = Vec::new();
    let mut n = 0;
    for s in trace["spans"].as_array().into_iter().flatten() {
        let (Some(name), Some(t)) = (s["name"].as_str(), s["t_ms"].as_f64()) else {
            continue;
        };
        let Some(sub) = name.strip_prefix("subsystem.") else {
            continue;
        };
        if t < fp {
            n += 1;
            if !allowed(sub, &patterns) {
                bad.push(format!("{sub} (t = {t:.1} ms)"));
            }
        }
    }
    report.note(format!(
        "{n} subsystem(s) initialized before first paint ({fp:.0} ms)"
    ));
    for b in bad {
        report.fail(format!(
            "subsystem '{b}' initialized before first paint and is not on tools/check-budgets/startup-allowlist.txt"
        ));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help") {
        eprintln!(
            "see the doc comment in tools/check-budgets/src/main.rs; all inputs are optional flags"
        );
        return ExitCode::from(2);
    }
    let all = |n: &str| -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == n)
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect()
    };
    let one = |n: &str| all(n).into_iter().next();
    let platform = match one("--platform").as_deref() {
        Some("macos") => Platform::Macos,
        Some("linux") => Platform::Linux,
        Some("windows") => Platform::Windows,
        Some(p) => {
            eprintln!("unknown platform {p}");
            return ExitCode::from(2);
        }
        None if cfg!(windows) => Platform::Windows,
        None if cfg!(target_os = "macos") => Platform::Macos,
        None => Platform::Linux,
    };
    let labels = one("--labels")
        .or_else(|| std::env::var("PR_LABELS").ok())
        .map(|l| l.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let ctx = Ctx {
        platform,
        reference: args.iter().any(|a| a == "--reference"),
        labels,
    };
    let mut report = Report::default();
    let baseline = one("--baseline").and_then(|p| read_json(&p, &mut report));
    let history = one("--history").and_then(|p| read_json(&p, &mut report));
    let mut sizes = BTreeMap::new();

    check_installers(
        &ctx,
        &all("--installer"),
        baseline.as_ref(),
        &mut sizes,
        &mut report,
    );
    if let Some(d) = one("--js-dist") {
        check_js(&ctx, &d, baseline.as_ref(), &mut sizes, &mut report);
    }
    if let Some(v) = one("--launch").and_then(|p| read_json(&p, &mut report)) {
        check_launch(&ctx, &v, history.as_ref(), &mut report);
    }
    if let Some(v) = one("--memory").and_then(|p| read_json(&p, &mut report)) {
        check_memory(&ctx, &v, history.as_ref(), &mut report);
    }
    if let Some(v) = one("--large-doc").and_then(|p| read_json(&p, &mut report)) {
        check_large(&ctx, &v, &mut report);
    }
    if let Some(p) = one("--startup-trace")
        && let Some(v) = read_json(&p, &mut report)
    {
        let al = one("--startup-allowlist").unwrap_or_else(|| {
            repo_root(&std::env::current_dir().unwrap())
                .join("tools/check-budgets/startup-allowlist.txt")
                .to_string_lossy()
                .into_owned()
        });
        check_startup(&v, &al, &mut report);
    }
    if let Some(out) = one("--write-sizes") {
        let artifacts: BTreeMap<&String, &u64> = sizes
            .iter()
            .filter(|(k, _)| *k != "js_initial_gzip")
            .collect();
        let v = json!({"artifacts": artifacts, "js_initial_gzip": sizes.get("js_initial_gzip")});
        if let Err(e) = std::fs::write(&out, serde_json::to_string_pretty(&v).unwrap()) {
            report.fail(format!("{out}: {e}"));
        }
    }
    ExitCode::from(report.finish("check-budgets") as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_initial_scripts() {
        let html = r#"<script type="module" crossorigin src="/assets/index-a.js"></script><link rel="modulepreload" href="/assets/vendor-b.js"><link rel="stylesheet" href="/assets/x.css">"#;
        assert_eq!(
            initial_scripts(html),
            vec!["assets/index-a.js", "assets/vendor-b.js"]
        );
    }

    #[test]
    fn median_and_allowlist() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0]), Some(1.5));
        let al = vec!["window".to_string(), "engine.*".to_string()];
        assert!(allowed("window", &al));
        assert!(allowed("engine.spawn", &al));
        assert!(allowed("engine", &al));
        assert!(!allowed("ocr", &al));
    }

    #[test]
    fn installer_kinds() {
        assert_eq!(
            kind_of("Papyrine_1.0_amd64.AppImage").map(|k| k.0),
            Some("appimage")
        );
        assert_eq!(kind_of("a.dmg").map(|k| k.1), Some(50 * MB));
        assert!(kind_of("notes.txt").is_none());
    }
}
