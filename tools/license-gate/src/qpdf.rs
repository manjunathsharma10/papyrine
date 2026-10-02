//! qpdf build-configuration gate: the CMake cache must select native crypto only.

use crate::Report;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn parse_cache(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("//") || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.split(':').next().unwrap_or(k).trim();
            m.insert(key.to_string(), v.trim().to_string());
        }
    }
    m
}

/// Finds CMake caches under `dir` that belong to a qpdf build.
pub fn find_caches(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 10 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                if !e.file_name().to_string_lossy().starts_with("CMakeFiles") {
                    walk(&p, depth + 1, out);
                }
            } else if e.file_name() == "CMakeCache.txt"
                && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("DEFAULT_CRYPTO"))
            {
                out.push(p);
            }
        }
    }
    if dir.is_file() {
        return vec![dir.to_path_buf()];
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort();
    out
}

pub fn check_cache(path: &Path, report: &mut Report) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            report.fail(format!("{}: {e}", path.display()));
            return;
        }
    };
    let c = parse_cache(&text);
    let want = |key: &str, val: &str, report: &mut Report| match c.get(key) {
        Some(v) if v.eq_ignore_ascii_case(val) => {}
        Some(v) => report.fail(format!("{}: {key}={v} (must be {val})", path.display())),
        None => report.fail(format!(
            "{}: {key} is not set (must be {val})",
            path.display()
        )),
    };
    want("DEFAULT_CRYPTO", "native", report);
    want("REQUIRE_CRYPTO_GNUTLS", "OFF", report);
    want("REQUIRE_CRYPTO_OPENSSL", "OFF", report);
    if c.get("REQUIRE_CRYPTO_NATIVE")
        .is_some_and(|v| v.eq_ignore_ascii_case("OFF"))
    {
        report.fail(format!("{}: REQUIRE_CRYPTO_NATIVE=OFF", path.display()));
    }
    // With implicit crypto on, qpdf links any provider it finds on the host.
    let implicit = c
        .get("USE_IMPLICIT_CRYPTO")
        .is_none_or(|v| v.eq_ignore_ascii_case("ON"));
    if implicit {
        for key in [
            "GNUTLS_LIBRARY",
            "OPENSSL_CRYPTO_LIBRARY",
            "OPENSSL_SSL_LIBRARY",
        ] {
            if let Some(v) = c.get(key)
                && !v.is_empty()
                && !v.ends_with("NOTFOUND")
            {
                report.fail(format!(
                    "{}: USE_IMPLICIT_CRYPTO is ON and {key}={v} was found; qpdf would link it (set USE_IMPLICIT_CRYPTO=OFF)",
                    path.display()
                ));
            }
        }
    }
}

/// Checks every qpdf CMake cache under `dir`. With `require`, finding none is a failure.
pub fn check(dir: &Path, require: bool, report: &mut Report) {
    let caches = find_caches(dir);
    if caches.is_empty() {
        if require {
            report.fail(format!(
                "no qpdf CMakeCache.txt found under {}",
                dir.display()
            ));
        } else {
            report.note(format!(
                "no qpdf CMake cache under {} (qpdf not built yet); skipped",
                dir.display()
            ));
        }
        return;
    }
    for c in &caches {
        check_cache(c, report);
    }
    report.note(format!("{} CMake cache(s) checked", caches.len()));
}
