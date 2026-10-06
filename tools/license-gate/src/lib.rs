//! Shared licence policy (ARCHITECTURE §12) and the gates built on it.

pub mod native;
pub mod qpdf;
pub mod spdx;

use std::path::{Path, PathBuf};

/// Licences accepted in shipped artifacts (ARCHITECTURE §12).
pub const ALLOWED: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "MPL-2.0",
    "Unicode-3.0",
    "FTL",
    "IJG",
    "libpng",
    "libpng-2.0",
    "BSL-1.0",
    "0BSD",
    "CC0-1.0",
];

/// Accepted only for entries marked `kind = "font"`.
pub const ALLOWED_FONT_ONLY: &[&str] = &["OFL-1.1"];

/// Accepted only in `third_party/native.toml` (permissive licences without an SPDX id).
pub const ALLOWED_NATIVE_ONLY: &[&str] = &["LicenseRef-AGG-2.3"];

/// Exceptions that may accompany an allowed licence.
pub const ALLOWED_EXCEPTIONS: &[&str] = &["LLVM-exception"];

/// Names that must never appear in a shipped inventory (case-insensitive substring).
pub const BANNED_NAMES: &[&str] = &[
    "mupdf",
    "ghostscript",
    "jbig2dec",
    "dssim",
    "poppler",
    "gnutls",
    "openssl",
    "libheif",
];

/// Accumulates failures so a gate can report everything at once.
#[derive(Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn fail(&mut self, msg: impl Into<String>) {
        self.failures.push(msg.into());
    }
    pub fn note(&mut self, msg: impl Into<String>) {
        self.notes.push(msg.into());
    }
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    /// Prints the report and returns the process exit code.
    pub fn finish(self, gate: &str) -> i32 {
        for n in &self.notes {
            println!("{gate}: {n}");
        }
        if self.ok() {
            println!("{gate}: PASS");
            return 0;
        }
        let gha = std::env::var_os("GITHUB_ACTIONS").is_some();
        for f in &self.failures {
            if gha {
                println!("::error title={gate}::{f}");
            }
            eprintln!("{gate}: FAIL: {f}");
        }
        eprintln!("{gate}: {} violation(s)", self.failures.len());
        1
    }
}

pub fn banned_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    BANNED_NAMES.iter().copied().find(|b| lower.contains(b))
}

/// Walks up from `start` to the directory holding `Cargo.toml` with `[workspace]`.
pub fn repo_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if let Ok(s) = std::fs::read_to_string(dir.join("Cargo.toml"))
            && s.contains("[workspace]")
        {
            return dir;
        }
        if !dir.pop() {
            return start.to_path_buf();
        }
    }
}

/// Simple glob supporting `*` (any run of characters). Case-insensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// JS gate: checks `pnpm licenses list --prod --json` output.
pub fn check_js(json: &str, report: &mut Report) {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            report.fail(format!("cannot parse pnpm licenses JSON: {e}"));
            return;
        }
    };
    let Some(map) = v.as_object() else {
        // pnpm prints `{}` or a message when there are no production dependencies.
        report.note("no production dependencies");
        return;
    };
    let mut count = 0;
    for (license, pkgs) in map {
        for pkg in pkgs.as_array().into_iter().flatten() {
            count += 1;
            let name = pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let versions = pkg
                .get("versions")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            if let Some(b) = banned_name(name) {
                report.fail(format!(
                    "JS package {name}@{versions} matches banned name '{b}'"
                ));
            }
            match spdx::check(license, false) {
                Ok(()) => {}
                Err(e) => report.fail(format!(
                    "JS package {name}@{versions} has licence '{license}': {e}"
                )),
            }
        }
    }
    report.note(format!("{count} production JS package(s) checked"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("libssl*", "libssl.so.3"));
        assert!(glob_match("*poppler*", "LibPoppler.dylib"));
        assert!(glob_match(
            "api-ms-win-*",
            "api-ms-win-core-file-l1-1-0.dll"
        ));
        assert!(!glob_match("libgs.*", "libgsf.so"));
        assert!(glob_match("kernel32.dll", "KERNEL32.DLL"));
    }

    #[test]
    fn agg_only_in_native_manifest() {
        assert!(spdx::check("LicenseRef-AGG-2.3", false).is_err());
        assert!(spdx::check_native("LicenseRef-AGG-2.3", false).is_ok());
    }

    #[test]
    fn banned_names_are_case_insensitive() {
        assert_eq!(banned_name("GnuTLS-sys"), Some("gnutls"));
        assert_eq!(banned_name("qpdf"), None);
    }

    #[test]
    fn js_gate_flags_copyleft_and_banned() {
        let mut r = Report::default();
        check_js(
            r#"{"MIT":[{"name":"react","versions":["18.0.0"]}],"GPL-3.0":[{"name":"x","versions":["1.0.0"]}]}"#,
            &mut r,
        );
        assert_eq!(r.failures.len(), 1);
        let mut r = Report::default();
        check_js(r#"{"MIT":[{"name":"mupdf","versions":["1.0.0"]}]}"#, &mut r);
        assert_eq!(r.failures.len(), 1);
    }

    #[test]
    fn qpdf_cache_parsing() {
        let c = qpdf::parse_cache("// comment\nDEFAULT_CRYPTO:STRING=native\nX:BOOL=OFF\n");
        assert_eq!(c["DEFAULT_CRYPTO"], "native");
        assert_eq!(c["X"], "OFF");
    }
}
