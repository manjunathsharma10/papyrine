//! The v0.1.x CLI skeleton: `papyrine info [--json] [--password PW] FILE`.
//!
//! Runs in-process (the CLI is the user's own command, not a sandboxed child) on the same
//! model code the engine uses.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use papyrine_cos::{Error as CosError, OpenOptions};
use papyrine_model::Model;
use serde_json::{Value, json};

const USAGE: &str = "usage: papyrine info [--json] [--password PASSWORD] FILE\n";

/// Exit codes: 0 ok, 1 the file could not be read, 2 usage, 3 a password is needed.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let (_, args) = crate::roles::strip_role_arg(args);
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("info") => info(it.as_slice(), out, err),
        Some("--version" | "-V") => {
            let _ = writeln!(out, "papyrine {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => {
            let _ = out.write_all(USAGE.as_bytes());
            ExitCode::SUCCESS
        }
        _ => {
            let _ = err.write_all(USAGE.as_bytes());
            ExitCode::from(2)
        }
    }
}

fn info(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> ExitCode {
    let (mut json_out, mut password, mut file) = (false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json_out = true,
            "--password" => match it.next() {
                Some(p) => password = Some(p.clone()),
                None => {
                    let _ = err.write_all(USAGE.as_bytes());
                    return ExitCode::from(2);
                }
            },
            s if s.starts_with('-') => {
                let _ = writeln!(err, "papyrine: unknown option {s}");
                let _ = err.write_all(USAGE.as_bytes());
                return ExitCode::from(2);
            }
            _ if file.is_none() => file = Some(a.clone()),
            _ => {
                let _ = err.write_all(USAGE.as_bytes());
                return ExitCode::from(2);
            }
        }
    }
    let Some(file) = file else {
        let _ = err.write_all(USAGE.as_bytes());
        return ExitCode::from(2);
    };
    match collect(Path::new(&file), password) {
        Ok(v) => {
            if json_out {
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                );
            } else {
                let _ = out.write_all(render_text(&v).as_bytes());
            }
            ExitCode::SUCCESS
        }
        Err(CliError::Password) => {
            let _ = writeln!(
                err,
                "papyrine: {file}: a password is required (or the one given is wrong)"
            );
            ExitCode::from(3)
        }
        Err(CliError::Other(m)) => {
            let _ = writeln!(err, "papyrine: {file}: {m}");
            ExitCode::from(1)
        }
    }
}

enum CliError {
    Password,
    Other(String),
}

impl From<CosError> for CliError {
    fn from(e: CosError) -> Self {
        match e {
            CosError::InvalidPassword => CliError::Password,
            e => CliError::Other(e.to_string()),
        }
    }
}

fn collect(path: &Path, password: Option<String>) -> Result<Value, CliError> {
    let size = std::fs::metadata(path)
        .map_err(|e| CliError::Other(e.to_string()))?
        .len();
    let opts = match password {
        Some(p) => OpenOptions::with_password(p),
        None => OpenOptions::default(),
    };
    let m = Model::open_path(path, &opts)?;
    let n = m.page_count()?;
    let info = m.info()?;
    let cat = m.catalog_info()?;
    let sec = m.security()?;
    let sigs = m.signatures()?;
    let form = m.form()?;
    let outlines = m.outlines()?;
    let first = if n > 0 { Some(m.page(0)?) } else { None };
    let mut annotations = 0usize;
    let mut rotated = 0usize;
    for i in 0..n {
        let p = m.page(i)?;
        annotations += p.annotation_count;
        rotated += usize::from(p.rotate != 0);
    }
    let log = m.document().repair_log();
    Ok(json!({
        "file": path.to_string_lossy(),
        "size_bytes": size,
        "pdf_version": cat.version,
        "pages": n,
        "first_page": first.map(|p| {
            let s = p.display_size();
            json!({"width_pt": s.w, "height_pt": s.h, "rotation": p.rotate})
        }),
        "rotated_pages": rotated,
        "title": info.title, "author": info.author, "subject": info.subject,
        "keywords": info.keywords, "creator": info.creator, "producer": info.producer,
        "created": info.creation_date_raw, "modified": info.mod_date_raw,
        "linearized": cat.is_linearized,
        "tagged": cat.is_marked,
        "language": cat.language,
        "encrypted": sec.encrypted,
        "encryption": sec.algorithm,
        "form_fields": form.terminal_fields().count(),
        "has_xfa": !matches!(form.xfa, papyrine_model::XfaKind::None),
        "annotations": annotations,
        "bookmarks": outlines.total(),
        "signed": sigs.is_signed(),
        "signature_fields": sigs.fields.len(),
        "repaired": !log.is_empty(),
        "warnings": log.iter().take(20).map(ToString::to_string).collect::<Vec<_>>(),
    }))
}

fn render_text(v: &Value) -> String {
    let mut s = String::new();
    let Some(o) = v.as_object() else { return s };
    let w = o.keys().map(String::len).max().unwrap_or(0);
    for (k, val) in o {
        let text = match val {
            Value::Null => continue,
            Value::String(t) => t.clone(),
            Value::Array(a) if a.is_empty() => continue,
            Value::Array(a) => a
                .iter()
                .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                .collect::<Vec<_>>()
                .join("; "),
            Value::Object(m) => m
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        };
        s.push_str(&format!("{k:<w$}  {text}\n"));
    }
    s
}
