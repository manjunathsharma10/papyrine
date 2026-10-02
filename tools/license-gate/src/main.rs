use license_gate::{Report, native, qpdf, repo_root};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: license-gate <js|native|qpdf|all> [options]
  js      [--dir apps/desktop] [--input pnpm-licenses.json]
  native  [--require-fetched]   (fail when licence files are not fetched)
  qpdf    [--build-dir <dir|CMakeCache.txt>] [--require]
  all     runs js, native and qpdf with defaults
common: --root <repo> (default: auto-detected)";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn run_js(root: &std::path::Path, args: &[String]) -> Report {
    let mut r = Report::default();
    if let Some(input) = arg(args, "--input") {
        match std::fs::read_to_string(&input) {
            Ok(s) => license_gate::check_js(&s, &mut r),
            Err(e) => r.fail(format!("{input}: {e}")),
        }
        return r;
    }
    let dir = root.join(arg(args, "--dir").unwrap_or_else(|| "apps/desktop".into()));
    if !dir.join("package.json").exists() {
        r.note(format!(
            "{} has no package.json yet; skipped",
            dir.display()
        ));
        return r;
    }
    if !dir.join("node_modules").exists() {
        r.fail(format!("{}: run `pnpm install` first", dir.display()));
        return r;
    }
    let out = Command::new(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" })
        .args(["licenses", "list", "--prod", "--json"])
        .current_dir(&dir)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            license_gate::check_js(&String::from_utf8_lossy(&o.stdout), &mut r)
        }
        Ok(o) => r.fail(format!(
            "pnpm licenses failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => r.fail(format!("cannot run pnpm: {e}")),
    }
    r
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let root = arg(&args, "--root")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root(&std::env::current_dir().unwrap()));
    let qpdf_dir = || {
        arg(&args, "--build-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
    };
    let require_fetched = args.iter().any(|a| a == "--require-fetched");
    let require = args.iter().any(|a| a == "--require");
    let mut code = 0;
    match cmd {
        "js" => code = run_js(&root, &args).finish("license-gate/js"),
        "native" => {
            let mut r = Report::default();
            native::check(&root, require_fetched, &mut r);
            code = r.finish("license-gate/native");
        }
        "qpdf" => {
            let mut r = Report::default();
            qpdf::check(&qpdf_dir(), require, &mut r);
            code = r.finish("license-gate/qpdf");
        }
        "all" => {
            code |= run_js(&root, &args).finish("license-gate/js");
            let mut r = Report::default();
            native::check(&root, require_fetched, &mut r);
            code |= r.finish("license-gate/native");
            let mut r = Report::default();
            qpdf::check(&qpdf_dir(), require, &mut r);
            code |= r.finish("license-gate/qpdf");
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    ExitCode::from(code as u8)
}
