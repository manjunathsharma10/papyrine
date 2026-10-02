//! Bundle inspection gate (ARCHITECTURE section 12, ADR-015).
//!
//! usage: inspect-bundle <artifact> [--manifest third_party/native.toml]
//!                       [--config allowlist.toml] [--list]

use goblin::Object;
use goblin::mach::{Mach, SingleArch};
use license_gate::native::Manifest;
use license_gate::{Report, glob_match, repo_root};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const DEFAULT_CONFIG: &str = include_str!("../allowlist.toml");

#[derive(Deserialize)]
struct Config {
    banned: Vec<String>,
    banned_symbol_prefixes: Vec<String>,
    first_party: Vec<String>,
    #[serde(default)]
    banned_waivers: Vec<String>,
    #[serde(default)]
    installer_internal: Vec<String>,
    macos: Macos,
    linux: Sys,
    windows: Sys,
}
#[derive(Deserialize)]
struct Macos {
    system_prefixes: Vec<String>,
}
#[derive(Deserialize)]
struct Sys {
    system: Vec<String>,
}

struct Extracted {
    dir: PathBuf,
    cleanup: Vec<(String, Vec<String>)>,
    temp: Option<PathBuf>,
}

impl Drop for Extracted {
    fn drop(&mut self) {
        for (prog, args) in &self.cleanup {
            let _ = Command::new(prog).args(args).output();
        }
        if let Some(t) = &self.temp {
            let _ = std::fs::remove_dir_all(t);
        }
    }
}

fn run(prog: &str, args: &[&str], cwd: Option<&Path>) -> Result<(), String> {
    let mut c = Command::new(prog);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let o = c.output().map_err(|e| format!("cannot run {prog}: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{prog} failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

fn materialize(artifact: &Path) -> Result<Extracted, String> {
    if artifact.is_dir() || artifact.extension().is_some_and(|e| e == "a" || e == "lib") {
        return Ok(Extracted {
            dir: artifact.to_path_buf(),
            cleanup: vec![],
            temp: None,
        });
    }
    let name = artifact
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_ascii_lowercase();
    let tmp = std::env::temp_dir().join(format!("inspect-bundle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let abs =
        std::fs::canonicalize(artifact).map_err(|e| format!("{}: {e}", artifact.display()))?;
    // msiexec and 7z reject Windows verbatim paths (\\?\C:\...).
    let plain = |p: &Path| {
        let s = p.to_string_lossy().into_owned();
        s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s)
    };
    let (a, t) = (plain(&abs), plain(&tmp));
    let mut ex = Extracted {
        dir: tmp.clone(),
        cleanup: vec![],
        temp: Some(tmp.clone()),
    };
    if name.ends_with(".dmg") {
        let mnt = tmp.join("mnt");
        std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
        let m = mnt.to_string_lossy().into_owned();
        run(
            "hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-noverify",
                "-mountpoint",
                &m,
                &a,
            ],
            None,
        )?;
        ex.cleanup
            .push(("hdiutil".into(), vec!["detach".into(), "-force".into(), m]));
        ex.dir = mnt;
    } else if name.ends_with(".deb") {
        run("dpkg-deb", &["-x", &a, &t], None).or_else(|_| {
            run("ar", &["x", &a], Some(&tmp))?;
            let data = std::fs::read_dir(&tmp)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .find(|e| e.file_name().to_string_lossy().starts_with("data.tar"))
                .ok_or("no data.tar in .deb")?;
            run(
                "tar",
                &["-xf", &data.path().to_string_lossy(), "-C", &t],
                None,
            )
        })?;
    } else if name.ends_with(".rpm") {
        run("bsdtar", &["-xf", &a, "-C", &t], None).or_else(|_| {
            run(
                "sh",
                &["-c", &format!("rpm2cpio '{a}' | cpio -idm --quiet")],
                Some(&tmp),
            )
        })?;
    } else if name.ends_with(".appimage") {
        let copy = tmp.join("app.AppImage");
        std::fs::copy(&abs, &copy).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755));
        }
        run(&copy.to_string_lossy(), &["--appimage-extract"], Some(&tmp))?;
        ex.dir = tmp.join("squashfs-root");
    } else if name.ends_with(".msi") {
        if cfg!(windows) {
            let target = format!("TARGETDIR={t}\\x");
            run("msiexec", &["/a", &a, "/qn", &target], None)?;
        } else {
            run("7z", &["x", &format!("-o{t}"), "-y", &a], None)?;
        }
    } else if name.ends_with(".exe") || name.ends_with(".zip") || name.ends_with(".7z") {
        run("7z", &["x", &format!("-o{t}"), "-y", &a], None)
            .or_else(|_| run("7za", &["x", &format!("-o{t}"), "-y", &a], None))
            .or_else(|_| run("tar", &["-xf", &a, "-C", &t], None))?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar.zst") {
        run("tar", &["-xf", &a, "-C", &t], None)?;
    } else {
        return Err(format!(
            "unsupported artifact type: {name} (flatpak bundles are not inspectable here)"
        ));
    }
    Ok(ex)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if md.is_dir() {
            walk(&p, out);
        } else if md.is_file() {
            out.push(p);
        }
    }
}

#[derive(Default)]
struct Facts {
    libs: Vec<String>,
    symbols: Vec<String>,
}

fn mach_facts(m: &goblin::mach::MachO, f: &mut Facts) {
    f.libs.extend(m.libs.iter().skip(1).map(|s| s.to_string()));
    f.symbols
        .extend(m.symbols().flatten().map(|(n, _)| n.to_string()));
}

fn facts_of(bytes: &[u8], ext: &str) -> Option<Facts> {
    let magic = bytes.get(..4)?;
    let looks_binary = magic == b"\x7fELF"
        || (magic[..2] == *b"MZ" && ["exe", "dll", "sys", "node", "pyd"].contains(&ext))
        || magic == b"!<ar"
        || matches!(
            magic,
            [0xfe, 0xed, 0xfa, 0xce | 0xcf]
                | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
        );
    if !looks_binary {
        return None;
    }
    let mut f = Facts::default();
    match Object::parse(bytes).ok()? {
        Object::Elf(e) => {
            f.libs.extend(e.libraries.iter().map(|s| s.to_string()));
            f.symbols.extend(
                e.dynsyms
                    .iter()
                    .filter_map(|s| e.dynstrtab.get_at(s.st_name))
                    .map(String::from),
            );
            f.symbols.extend(
                e.syms
                    .iter()
                    .filter_map(|s| e.strtab.get_at(s.st_name))
                    .map(String::from),
            );
        }
        Object::PE(p) => {
            f.libs.extend(p.libraries.iter().map(|s| s.to_string()));
            f.symbols
                .extend(p.imports.iter().map(|i| i.name.to_string()));
            f.symbols
                .extend(p.exports.iter().filter_map(|x| x.name).map(String::from));
        }
        Object::Mach(Mach::Binary(m)) => mach_facts(&m, &mut f),
        Object::Mach(Mach::Fat(multi)) => {
            for arch in multi.into_iter().flatten() {
                match arch {
                    SingleArch::MachO(m) => mach_facts(&m, &mut f),
                    SingleArch::Archive(a) => f.symbols.extend(archive_symbols(&a)),
                }
            }
        }
        Object::Archive(a) => f.symbols.extend(archive_symbols(&a)),
        _ => return None,
    }
    f.libs.sort();
    f.libs.dedup();
    Some(f)
}

fn archive_symbols(a: &goblin::archive::Archive) -> Vec<String> {
    a.summarize()
        .into_iter()
        .flat_map(|(_, _, syms)| syms.into_iter().map(String::from))
        .collect()
}

fn is_shared_lib_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".dylib") || n.ends_with(".dll") || n.contains(".so.") || n.ends_with(".so")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |n: &str| {
        args.iter()
            .position(|a| a == n)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let Some(artifact) = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
    else {
        eprintln!("usage: inspect-bundle <artifact> [--manifest FILE] [--config FILE] [--list]");
        return ExitCode::from(2);
    };
    let root = repo_root(&std::env::current_dir().unwrap());
    let cfg_text = match arg("--config") {
        Some(p) => std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}")),
        None => DEFAULT_CONFIG.to_string(),
    };
    let cfg: Config = toml::from_str(&cfg_text).expect("invalid allowlist config");
    let manifest_path = arg("--manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("third_party/native.toml"));
    let mut report = Report::default();
    let provided: Vec<String> = if manifest_path.exists() {
        match Manifest::load(&manifest_path) {
            Ok(m) => m.library.iter().flat_map(|l| l.provides.clone()).collect(),
            Err(e) => {
                report.fail(format!("cannot read native manifest: {e}"));
                Vec::new()
            }
        }
    } else {
        report.note("no native.toml; only first-party and system libraries are allowed");
        Vec::new()
    };

    let ex = match materialize(&artifact) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("inspect-bundle: FAIL: {e}");
            return ExitCode::from(1);
        }
    };
    let mut files = Vec::new();
    if ex.dir.is_file() {
        files.push(ex.dir.clone());
    } else {
        walk(&ex.dir, &mut files);
    }
    let list = args.iter().any(|a| a == "--list");
    let total: u64 = files
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum();
    println!(
        "inspect-bundle: {} file(s), {:.1} MB in {}",
        files.len(),
        total as f64 / 1e6,
        artifact.display()
    );

    let is_appimage = artifact
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with(".appimage");
    let waived = |name: &str| cfg.banned_waivers.iter().any(|w| glob_match(w, name));
    let allowed_bundled = |name: &str| {
        provided
            .iter()
            .chain(cfg.first_party.iter())
            .any(|p| glob_match(p, name))
    };
    let mut seen_libs: std::collections::BTreeSet<(String, String)> = Default::default();

    for f in &files {
        let rel = f
            .strip_prefix(&ex.dir)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let base = f.file_name().unwrap().to_string_lossy().into_owned();
        if list {
            println!(
                "  {rel}  ({} bytes)",
                std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)
            );
        }
        if !waived(&base)
            && let Some(p) = cfg.banned.iter().find(|p| glob_match(p, &base))
        {
            report.fail(format!("banned file '{rel}' (matches '{p}')"));
        }
        let internal_file = cfg.installer_internal.iter().any(|p| glob_match(p, &rel));
        let ext = f
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if is_shared_lib_name(&base) {
            let appimage_stack =
                is_appimage && cfg.linux.system.iter().any(|p| glob_match(p, &base));
            if !allowed_bundled(&base) && !appimage_stack && !internal_file {
                report.fail(format!(
                    "bundled library '{rel}' is not listed in native.toml `provides` or first_party"
                ));
            }
        }
        let Ok(md) = std::fs::metadata(f) else {
            continue;
        };
        if md.len() > 1 << 30 {
            continue;
        }
        let mut head = [0u8; 8];
        let Ok(mut fh) = std::fs::File::open(f) else {
            continue;
        };
        let n = std::io::Read::read(&mut fh, &mut head).unwrap_or(0);
        if n < 4 {
            continue;
        }
        drop(fh);
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let Some(facts) = facts_of(&bytes, &ext) else {
            continue;
        };
        for lib in &facts.libs {
            if !seen_libs.insert((rel.clone(), lib.clone())) {
                continue;
            }
            let libname = lib.rsplit(['/', '\\']).next().unwrap_or(lib);
            if !waived(libname)
                && let Some(p) = cfg.banned.iter().find(|p| glob_match(p, libname))
            {
                report.fail(format!(
                    "{rel} links banned library '{lib}' (matches '{p}')"
                ));
                continue;
            }
            let ok = if lib.starts_with('/')
                && cfg.macos.system_prefixes.iter().any(|p| lib.starts_with(p))
            {
                true
            } else if lib.starts_with('@') {
                allowed_bundled(libname)
            } else if lib.starts_with('/') && !libname.contains(".so") {
                false
            } else {
                allowed_bundled(libname)
                    || cfg.linux.system.iter().any(|p| glob_match(p, libname))
                    || cfg.windows.system.iter().any(|p| glob_match(p, libname))
            };
            if !ok && !internal_file {
                report.fail(format!(
                    "{rel} links '{lib}', which is not in native.toml or the system allowlist"
                ));
            }
        }
        let hits: Vec<&String> = facts
            .symbols
            .iter()
            .filter(|s| {
                let t = s.trim_start_matches('_');
                cfg.banned_symbol_prefixes
                    .iter()
                    .any(|p| t.starts_with(p.as_str()))
            })
            .collect();
        if !hits.is_empty() {
            let ex: Vec<&str> = hits.iter().take(3).map(|s| s.as_str()).collect();
            report.fail(format!(
                "{rel} contains {} GnuTLS/OpenSSL symbol(s), e.g. {}",
                hits.len(),
                ex.join(", ")
            ));
        }
    }
    ExitCode::from(report.finish("inspect-bundle") as u8)
}
