//! `third_party/native.toml`: the manifest of vendored native code.
//!
//! ```toml
//! [[library]]
//! name = "qpdf"
//! version = "12.4.0"
//! spdx = "Apache-2.0"                  # SPDX expression
//! dir = "qpdf"                         # directory under third_party/ (default: name)
//! license_files = ["qpdf/LICENSE.txt"] # relative to third_party/
//! provides = ["libqpdf*"]              # optional: library file names this ships as
//! credit = "..."                       # optional: required verbatim credit line
//! kind = "font"                        # optional: permits OFL-1.1
//! parent = "pdfium"                    # optional: sub-licence of another entry (no dir)
//! ```
//!
//! `license_files` are relative to the fetched source tree
//! (`third_party/cache/src/<name>-<version>/`), falling back to `third_party/<dir>/`.
//! Fetched trees are absent in a clean checkout; `--require-fetched` makes that a failure.

use crate::{Report, banned_name, spdx};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(alias = "license")]
    pub spdx: String,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default, alias = "notice_files", alias = "licenses")]
    pub license_files: Vec<String>,
    #[serde(default, alias = "libs")]
    pub provides: Vec<String>,
    #[serde(default)]
    pub credit: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

impl Library {
    /// Locates a licence file: fetched source tree first, then vendored dir.
    pub fn resolve_file(&self, root: &Path, file: &str) -> Option<std::path::PathBuf> {
        let tp = root.join("third_party");
        let mut cands = vec![
            tp.join("cache/src")
                .join(format!("{}-{}", self.name, self.version)),
        ];
        if let Some(p) = &self.parent {
            cands.push(tp.join("cache").join(p));
        }
        cands.push(tp.join(self.dir_name()));
        cands.push(tp);
        cands
            .into_iter()
            .map(|d| d.join(file))
            .find(|p| p.is_file())
    }

    fn is_fetched(&self, root: &Path) -> bool {
        let tp = root.join("third_party");
        tp.join("cache/src")
            .join(format!("{}-{}", self.name, self.version))
            .is_dir()
            || tp.join(self.dir_name()).is_dir()
            || self
                .parent
                .as_ref()
                .is_some_and(|p| tp.join("cache").join(p).is_dir())
    }

    pub fn is_font(&self) -> bool {
        self.kind.as_deref() == Some("font")
    }
    pub fn dir_name(&self) -> &str {
        self.dir.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub library: Vec<Library>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn collect_names(v: &toml::Value, out: &mut Vec<String>) {
    match v {
        toml::Value::Table(t) => {
            for (k, v) in t {
                if k == "name" {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_ascii_lowercase());
                    }
                } else {
                    collect_names(v, out);
                }
            }
        }
        toml::Value::Array(a) => a.iter().for_each(|x| collect_names(x, out)),
        _ => {}
    }
}

/// Names of test-only oracles from `tools/test-oracles.toml` (empty when absent).
pub fn oracle_names(root: &Path) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(root.join("tools/test-oracles.toml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Ok(v) = s.parse::<toml::Value>() {
        collect_names(&v, &mut out);
    }
    out
}

pub fn check(root: &Path, require_fetched: bool, report: &mut Report) {
    let tp = root.join("third_party");
    let manifest_path = tp.join("native.toml");
    let dirs: Vec<String> = match std::fs::read_dir(&tp) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "cache" && !n.starts_with('.'))
                .collect();
            v.sort();
            v
        }
        Err(_) => Vec::new(),
    };
    if !manifest_path.exists() {
        if dirs.is_empty() {
            report.note("no third_party/native.toml and nothing vendored; nothing to check");
        } else {
            report.fail(format!(
                "third_party/ has vendored directories ({}) but no native.toml",
                dirs.join(", ")
            ));
        }
        return;
    }
    let manifest = match Manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            report.fail(format!("cannot read manifest: {e}"));
            return;
        }
    };
    let oracles = oracle_names(root);
    let names: Vec<&str> = manifest.library.iter().map(|l| l.name.as_str()).collect();

    for lib in &manifest.library {
        let who = format!("native library '{}'", lib.name);
        if let Some(b) = banned_name(&lib.name) {
            report.fail(format!("{who} matches banned name '{b}'"));
        }
        if oracles.contains(&lib.name.to_ascii_lowercase()) {
            report.fail(format!(
                "{who} is a test-only oracle (tools/test-oracles.toml) and must not ship"
            ));
        }
        if lib.version.trim().is_empty() && lib.parent.is_none() {
            report.fail(format!("{who} has no version"));
        }
        if let Err(e) = spdx::check(&lib.spdx, lib.is_font()) {
            report.fail(format!("{who}: {e}"));
        }
        if let Some(p) = &lib.parent
            && !names.contains(&p.as_str())
        {
            report.fail(format!("{who}: parent '{p}' is not in the manifest"));
        }
        if lib.license_files.is_empty() {
            report.fail(format!("{who} lists no license_files"));
        }
        if let Some(h) = &lib.sha256
            && !(h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            report.fail(format!("{who}: sha256 is not 64 hex digits"));
        }
        if lib.url.is_some() && lib.sha256.is_none() {
            report.fail(format!("{who}: has a url but no sha256 pin"));
        }
        for f in &lib.license_files {
            if lib.resolve_file(root, f).is_some() {
                continue;
            }
            if require_fetched || lib.is_fetched(root) {
                report.fail(format!(
                    "{who}: license file '{f}' not found (run third_party/fetch)"
                ));
            } else {
                report.note(format!(
                    "{who}: '{f}' not fetched yet; presence not verified"
                ));
            }
        }
        if let Some(b) = lib.provides.iter().find_map(|p| banned_name(p)) {
            report.fail(format!(
                "{who}: provides a library matching banned name '{b}'"
            ));
        }
    }
    for d in &dirs {
        if !manifest
            .library
            .iter()
            .any(|l| l.parent.is_none() && l.dir_name() == d)
        {
            report.fail(format!(
                "third_party/{d} is not listed in native.toml (add a [[library]] with dir = \"{d}\")"
            ));
        }
        if let Some(b) = banned_name(d) {
            report.fail(format!("third_party/{d} matches banned name '{b}'"));
        }
    }
    report.note(format!(
        "{} manifest entr(ies), {} vendored dir(s) checked",
        manifest.library.len(),
        dirs.len()
    ));
}
