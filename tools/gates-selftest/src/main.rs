//! Gate self-test (ROADMAP 1.1 AC): every gate must FAIL on a deliberate violation
//! and PASS on clean input. Fixtures are built in a temp dir; nothing is committed.
//!
//! usage: gates-selftest [--require-all]   (--require-all turns SKIP into failure)

use license_gate::repo_root;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Harness {
    root: PathBuf,
    tmp: PathBuf,
    bin_dir: PathBuf,
    passed: u32,
    failed: u32,
    skipped: u32,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

impl Harness {
    fn bin(&self, pkg: &str) -> PathBuf {
        let p = self.bin_dir.join(exe(pkg));
        if !p.exists() {
            let ok = Command::new("cargo")
                .args(["build", "-q", "-p", pkg])
                .current_dir(&self.root)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!("selftest: cannot build {pkg}");
            }
        }
        p
    }

    fn run(&self, pkg: &str, args: &[&str]) -> Output {
        Command::new(self.bin(pkg))
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {pkg}: {e}"))
    }

    fn record(&mut self, name: &str, expect_fail: bool, out: &Output) {
        // A rejection must be a genuine violation (not a usage error, panic or missing binary).
        let failed = !out.status.success();
        let genuine = !failed
            || out.status.code().is_some_and(|c| c != 101 && c != 127)
                && !String::from_utf8_lossy(&out.stderr).contains("Usage:");
        if failed == expect_fail && genuine {
            self.passed += 1;
            println!(
                "ok    {name} ({})",
                if expect_fail { "rejected" } else { "accepted" }
            );
        } else {
            self.failed += 1;
            println!(
                "FAIL  {name}: expected the gate to {} but it {}\n{}{}",
                if expect_fail { "FAIL" } else { "PASS" },
                if failed { "failed" } else { "passed" },
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        println!("SKIP  {name}: {why}");
    }

    fn dir(&self, name: &str) -> PathBuf {
        let d = self.tmp.join(name);
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
}

fn write(p: &Path, s: impl AsRef<[u8]>) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn have(tool: &str, arg: &str) -> bool {
    Command::new(tool)
        .arg(arg)
        .output()
        .is_ok_and(|o| o.status.success())
}

// ---------------------------------------------------------------- fixtures

fn cargo_deny_fixtures(h: &mut Harness) {
    if !Command::new("cargo")
        .args(["deny", "--version"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        h.skip("cargo-deny fixtures", "cargo-deny is not installed");
        return;
    }
    let cfg = s(&h.root.join("deny.toml"));
    let mk = |h: &Harness, name: &str, dep_name: &str, dep_license: &str| -> PathBuf {
        let d = h.dir(name);
        write(
            &d.join("app/Cargo.toml"),
            format!(
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\nedition=\"2024\"\nlicense=\"MIT\"\npublish=false\n[dependencies]\n{dep_name}={{ path=\"../dep\", version=\"0.1.0\" }}\n"
            ),
        );
        write(&d.join("app/src/main.rs"), "fn main(){}\n");
        write(
            &d.join("dep/Cargo.toml"),
            format!(
                "[package]\nname=\"{dep_name}\"\nversion=\"0.1.0\"\nedition=\"2024\"\nlicense=\"{dep_license}\"\npublish=false\n"
            ),
        );
        write(&d.join("dep/src/lib.rs"), "");
        d.join("app/Cargo.toml")
    };
    for (name, dep, lic, fail) in [
        ("deny-gpl", "evil-gpl", "GPL-3.0-only", true),
        ("deny-agpl", "evil-agpl", "AGPL-3.0-or-later", true),
        ("deny-openssl", "openssl-sys", "MIT", true),
        ("deny-clean", "nice-dep", "MIT OR Apache-2.0", false),
    ] {
        let manifest = mk(h, name, dep, lic);
        let out = Command::new("cargo")
            .args([
                "deny",
                "--manifest-path",
                &s(&manifest),
                "--config",
                &cfg,
                "check",
                "licenses",
                "bans",
            ])
            .output()
            .unwrap();
        h.record(&format!("cargo-deny: {name}"), fail, &out);
    }
}

/// (name, native.toml, extra files, expect rejection)
type NativeCase<'a> = (&'a str, String, Vec<(&'a str, &'a str)>, bool);
/// (name, files, expect rejection)
type BundleCase<'a> = (&'a str, Vec<(&'a str, Vec<u8>)>, bool);

fn native_fixtures(h: &mut Harness) {
    let good = "[[library]]\nname=\"zlib\"\nversion=\"1.3.2\"\nspdx=\"Zlib\"\nlicense_files=[\"LICENSE\"]\nurl=\"https://example.com/z.tgz\"\nsha256=\"bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16\"\n";
    let cases: Vec<NativeCase> = vec![
        (
            "clean",
            good.into(),
            vec![("zlib/LICENSE", "zlib licence text")],
            false,
        ),
        (
            "banned-lib",
            format!(
                "{good}[[library]]\nname=\"libheif\"\nversion=\"1.0\"\nspdx=\"LGPL-3.0-or-later\"\nlicense_files=[\"LICENSE\"]\n"
            ),
            vec![("zlib/LICENSE", "x")],
            true,
        ),
        (
            "gpl-spdx",
            good.replace("Zlib\"", "GPL-3.0-only\""),
            vec![("zlib/LICENSE", "x")],
            true,
        ),
        (
            "unlisted-dir",
            good.into(),
            vec![("zlib/LICENSE", "x"), ("mystery/README", "x")],
            true,
        ),
        (
            "missing-license-file",
            good.into(),
            vec![("zlib/OTHER", "x")],
            true,
        ),
        (
            "unpinned",
            good.replace("sha256=", "#sha256="),
            vec![("zlib/LICENSE", "x")],
            true,
        ),
    ];
    for (name, manifest, files, fail) in cases {
        let d = h.dir(&format!("native-{name}"));
        write(&d.join("third_party/native.toml"), manifest);
        for (p, c) in files {
            write(&d.join("third_party").join(p), c);
        }
        let out = h.run("license-gate", &["native", "--root", &s(&d)]);
        h.record(&format!("native manifest: {name}"), fail, &out);
    }
    // Test oracle shipped by mistake.
    let d = h.dir("native-oracle");
    write(&d.join("third_party/native.toml"), good);
    write(&d.join("third_party/zlib/LICENSE"), "x");
    write(
        &d.join("tools/test-oracles.toml"),
        "[[oracle]]\nname=\"zlib\"\n",
    );
    let out = h.run("license-gate", &["native", "--root", &s(&d)]);
    h.record("native manifest: oracle in shipped inventory", true, &out);
}

fn js_fixtures(h: &mut Harness) {
    let d = h.dir("js");
    let clean = r#"{"MIT":[{"name":"react","versions":["18.3.1"],"paths":[]}],"(MIT OR Apache-2.0)":[{"name":"x","versions":["1.0.0"],"paths":[]}]}"#;
    let gpl = r#"{"GPL-3.0":[{"name":"copyleft-lib","versions":["1.0.0"],"paths":[]}],"MIT":[]}"#;
    let banned = r#"{"MIT":[{"name":"mupdf-wasm","versions":["1.0.0"],"paths":[]}]}"#;
    for (name, body, fail) in [
        ("clean", clean, false),
        ("gpl", gpl, true),
        ("banned-name", banned, true),
    ] {
        let f = d.join(format!("{name}.json"));
        write(&f, body);
        let out = h.run("license-gate", &["js", "--input", &s(&f)]);
        h.record(&format!("js licenses: {name}"), fail, &out);
    }
}

fn qpdf_fixtures(h: &mut Harness) {
    let base = "DEFAULT_CRYPTO:STRING=native\nREQUIRE_CRYPTO_GNUTLS:BOOL=OFF\nREQUIRE_CRYPTO_OPENSSL:BOOL=OFF\nUSE_IMPLICIT_CRYPTO:BOOL=OFF\n";
    let cases = [
        ("clean", base.to_string(), false),
        (
            "gnutls-default",
            base.replace(
                "DEFAULT_CRYPTO:STRING=native",
                "DEFAULT_CRYPTO:STRING=gnutls",
            ),
            true,
        ),
        (
            "require-gnutls",
            base.replace("GNUTLS:BOOL=OFF", "GNUTLS:BOOL=ON"),
            true,
        ),
        (
            "require-openssl",
            base.replace("OPENSSL:BOOL=OFF", "OPENSSL:BOOL=ON"),
            true,
        ),
        (
            "missing-keys",
            "DEFAULT_CRYPTO:STRING=native\n".to_string(),
            true,
        ),
        (
            "implicit-openssl-found",
            format!(
                "{}OPENSSL_CRYPTO_LIBRARY:FILEPATH=/usr/lib/libcrypto.so\n",
                base.replace("IMPLICIT_CRYPTO:BOOL=OFF", "IMPLICIT_CRYPTO:BOOL=ON")
            ),
            true,
        ),
    ];
    for (name, body, fail) in cases {
        let d = h.dir(&format!("qpdf-{name}"));
        write(&d.join("build/CMakeCache.txt"), body);
        let out = h.run(
            "license-gate",
            &["qpdf", "--build-dir", &s(&d), "--require"],
        );
        h.record(&format!("qpdf crypto config: {name}"), fail, &out);
    }
}

fn corpus_fixtures(h: &mut Harness) {
    let script = h.root.join("tools/check-no-corpus");
    if !script.exists() || !have("git", "--version") {
        h.skip("no-corpus fixtures", "tools/check-no-corpus or git missing");
        return;
    }
    let py = ["python3", "python"]
        .into_iter()
        .find(|p| have(p, "--version"));
    let Some(py) = py else {
        h.skip("no-corpus fixtures", "python not found");
        return;
    };
    for (name, file, content, fail) in [
        ("clean", "notes.txt", &b"hello"[..], false),
        ("committed-pdf", "docs/sample.pdf", &b"%PDF-1.4\n"[..], true),
        ("committed-key", "secrets/release.p12", &b"x"[..], true),
    ] {
        let d = h.dir(&format!("corpus-{name}"));
        write(&d.join(file), content);
        let git = |args: &[&str]| {
            Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&d)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["add", "-f", "."]);
        git(&["commit", "-q", "-m", "fixture"]);
        let out = Command::new(py)
            .arg(&script)
            .args(["--root", &s(&d)])
            .output()
            .unwrap();
        h.record(&format!("no-corpus: {name}"), fail, &out);
    }
}

fn notices_fixtures(h: &mut Harness) {
    let d = h.dir("notices");
    write(
        &d.join("Cargo.toml"),
        "[package]\nname=\"papyrine-app\"\nversion=\"0.0.1\"\nedition=\"2024\"\nlicense=\"MIT\"\npublish=false\n",
    );
    write(&d.join("src/main.rs"), "fn main(){}\n");
    let root = s(&d);
    let out = h.run("gen-notices", &["--root", &root]);
    h.record("notices: generate", false, &out);
    let out = h.run("gen-notices", &["--root", &root, "--check"]);
    h.record("notices: fresh file accepted", false, &out);
    let file = d.join("THIRD_PARTY_LICENSES.md");
    let text = fs::read_to_string(&file).unwrap_or_default();
    for credit in ["Independent JPEG Group", "The FreeType Project"] {
        if text.contains(credit) {
            h.passed += 1;
            println!("ok    notices: required credit '{credit}' present");
        } else {
            h.failed += 1;
            println!("FAIL  notices: required credit '{credit}' missing");
        }
    }
    write(&file, format!("{text}\nstale edit\n"));
    let out = h.run("gen-notices", &["--root", &root, "--check"]);
    h.record("notices: stale file", true, &out);
}

fn budget_fixtures(h: &mut Harness) {
    let d = h.dir("budgets");
    let run = |h: &Harness, args: &[&str]| h.run("check-budgets", args);
    let (ok, over) = (big_in(&d, "ok", 20), big_in(&d, "over", 51));
    h.record(
        "budgets: 20 MB dmg",
        false,
        &run(h, &["--platform", "macos", "--installer", &s(&ok)]),
    );
    h.record(
        "budgets: 51 MB dmg",
        true,
        &run(h, &["--platform", "macos", "--installer", &s(&over)]),
    );
    let appimage = big_in_named(&d, "appimage", "Papyrine.AppImage", 80);
    h.record(
        "budgets: 80 MB AppImage (limit 100)",
        false,
        &run(h, &["--platform", "linux", "--installer", &s(&appimage)]),
    );

    // 5% growth rule and its label override.
    let base = d.join("baseline.json");
    write(&base, r#"{"artifacts":{"dmg":20971520}}"#); // 20 MiB
    let grown = big_in_named(&d, "grown", "Papyrine.dmg", 22);
    h.record(
        "budgets: 10% growth vs main",
        true,
        &run(
            h,
            &[
                "--platform",
                "macos",
                "--installer",
                &s(&grown),
                "--baseline",
                &s(&base),
            ],
        ),
    );
    h.record(
        "budgets: 10% growth with size-increase-approved",
        false,
        &run(
            h,
            &[
                "--platform",
                "macos",
                "--installer",
                &s(&grown),
                "--baseline",
                &s(&base),
                "--labels",
                "size-increase-approved",
            ],
        ),
    );

    // JS bundle.
    for (name, bytes, fail) in [("js-small", 50_000usize, false), ("js-huge", 400_000, true)] {
        let dist = d.join(name);
        write(
            &dist.join("index.html"),
            "<html><script type=\"module\" src=\"/assets/index.js\"></script></html>",
        );
        write(&dist.join("assets/index.js"), pseudo_random(bytes));
        h.record(
            &format!("budgets: initial JS bundle {name}"),
            fail,
            &run(h, &["--js-dist", &s(&dist)]),
        );
    }

    // Launch / memory / large documents.
    let launch_ok = d.join("launch_ok.json");
    write(&launch_ok, r#"{"cold_ms":900,"warm_ms":300}"#);
    let launch_bad = d.join("launch_bad.json");
    write(&launch_bad, r#"{"cold_ms":2600,"warm_ms":300}"#);
    h.record(
        "budgets: cold launch 0.9 s",
        false,
        &run(h, &["--launch", &s(&launch_ok)]),
    );
    h.record(
        "budgets: cold launch 2.6 s",
        true,
        &run(h, &["--launch", &s(&launch_bad)]),
    );
    let hist = d.join("history.json");
    write(
        &hist,
        r#"{"cold_ms":[700,720,710],"idle_mb":[120,121,119]}"#,
    );
    h.record(
        "budgets: cold launch regressed >15% vs median",
        true,
        &run(h, &["--launch", &s(&launch_ok), "--history", &s(&hist)]),
    );
    let mem_ok = d.join("mem_ok.json");
    write(&mem_ok, r#"{"idle_mb":120}"#);
    let mem_bad = d.join("mem_bad.json");
    write(&mem_bad, r#"{"idle_mb":180}"#);
    h.record(
        "budgets: idle memory 120 MB (macOS)",
        false,
        &run(h, &["--platform", "macos", "--memory", &s(&mem_ok)]),
    );
    h.record(
        "budgets: idle memory 180 MB (macOS)",
        true,
        &run(h, &["--platform", "macos", "--memory", &s(&mem_bad)]),
    );
    h.record(
        "budgets: idle memory 180 MB (Windows, trend only)",
        false,
        &run(h, &["--platform", "windows", "--memory", &s(&mem_bad)]),
    );
    let large_ok = d.join("large_ok.json");
    write(
        &large_ok,
        r#"[{"file":"2000p","peak_mb":380,"settled_mb":200,"engine_mb":100,"renderer_mb":90}]"#,
    );
    let large_bad = d.join("large_bad.json");
    write(
        &large_bad,
        r#"[{"file":"2000p","peak_mb":450,"settled_mb":200,"engine_mb":100,"renderer_mb":90}]"#,
    );
    h.record(
        "budgets: large doc within budget",
        false,
        &run(h, &["--platform", "linux", "--large-doc", &s(&large_ok)]),
    );
    h.record(
        "budgets: large doc peak 450 MB",
        true,
        &run(h, &["--platform", "linux", "--large-doc", &s(&large_bad)]),
    );

    // Startup trace: subsystems before first paint vs the allowlist.
    let al = h.root.join("tools/check-budgets/startup-allowlist.txt");
    for (name, trace, fail) in [
        (
            "clean",
            r#"{"spans":[{"name":"subsystem.window","t_ms":2},{"name":"subsystem.ocr","t_ms":300}],"first_paint_ms":120}"#,
            false,
        ),
        (
            "ocr-before-paint",
            r#"{"spans":[{"name":"subsystem.window","t_ms":2},{"name":"subsystem.ocr","t_ms":50}],"first_paint_ms":120}"#,
            true,
        ),
        (
            "no-first-paint",
            r#"{"spans":[{"name":"subsystem.window","t_ms":2}]}"#,
            true,
        ),
    ] {
        let f = d.join(format!("trace-{name}.json"));
        write(&f, trace);
        h.record(
            &format!("startup trace: {name}"),
            fail,
            &run(
                h,
                &["--startup-trace", &s(&f), "--startup-allowlist", &s(&al)],
            ),
        );
    }
}

fn big_in(d: &Path, sub: &str, mib: u64) -> PathBuf {
    big_in_named(d, sub, "Papyrine.dmg", mib)
}

fn big_in_named(d: &Path, sub: &str, name: &str, mib: u64) -> PathBuf {
    let dir = d.join(sub);
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::File::create(&p)
        .unwrap()
        .set_len(mib * 1024 * 1024)
        .unwrap();
    p
}

fn pseudo_random(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Minimal ELF64 shared object whose dynamic section lists `needed` libraries.
fn elf_with_needed(needed: &[&str]) -> Vec<u8> {
    let mut dynstr = vec![0u8];
    let mut offs = Vec::new();
    for n in needed {
        offs.push(dynstr.len() as u64);
        dynstr.extend_from_slice(n.as_bytes());
        dynstr.push(0);
    }
    let phoff = 64u64;
    let ph_size = 56u64;
    let dynstr_off = phoff + 2 * ph_size;
    let dyn_off = (dynstr_off + dynstr.len() as u64).div_ceil(8) * 8;
    let n_dyn = needed.len() as u64 + 3;
    let total = dyn_off + n_dyn * 16;
    let mut b = Vec::new();
    b.extend_from_slice(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    b.extend_from_slice(&3u16.to_le_bytes()); // ET_DYN
    b.extend_from_slice(&62u16.to_le_bytes()); // x86-64
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes()); // entry
    b.extend_from_slice(&phoff.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes()); // shoff
    b.extend_from_slice(&0u32.to_le_bytes()); // flags
    b.extend_from_slice(&64u16.to_le_bytes());
    b.extend_from_slice(&(ph_size as u16).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&64u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    let phdr = |ty: u32, flags: u32, off: u64, size: u64, align: u64| {
        let mut p = Vec::new();
        p.extend_from_slice(&ty.to_le_bytes());
        p.extend_from_slice(&flags.to_le_bytes());
        for v in [off, off, off, size, size, align] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p
    };
    b.extend(phdr(1, 4, 0, total, 0x1000)); // PT_LOAD, vaddr == file offset
    b.extend(phdr(2, 4, dyn_off, n_dyn * 16, 8)); // PT_DYNAMIC
    b.extend_from_slice(&dynstr);
    b.resize(dyn_off as usize, 0);
    let mut dynent = |tag: u64, val: u64| {
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&val.to_le_bytes());
    };
    for o in offs {
        dynent(1, o); // DT_NEEDED
    }
    dynent(5, dynstr_off); // DT_STRTAB
    dynent(10, dynstr.len() as u64); // DT_STRSZ
    dynent(0, 0);
    b
}

/// GNU-format static archive with a symbol table naming `symbols`.
fn ar_with_symbols(symbols: &[&str]) -> Vec<u8> {
    let names: Vec<u8> = symbols.iter().flat_map(|s| s.bytes().chain([0])).collect();
    let sym_size = 4 + 4 * symbols.len() + names.len();
    let pad = sym_size % 2;
    let member_off = 8 + 60 + sym_size + pad;
    let mut sym = Vec::new();
    sym.extend_from_slice(&(symbols.len() as u32).to_be_bytes());
    for _ in symbols {
        sym.extend_from_slice(&(member_off as u32).to_be_bytes());
    }
    sym.extend_from_slice(&names);
    let header = |name: &str, size: usize| {
        format!(
            "{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n",
            0, 0, 0, 100644
        )
    };
    let mut out = b"!<arch>\n".to_vec();
    out.extend_from_slice(header("/", sym_size).as_bytes());
    out.extend_from_slice(&sym);
    if pad == 1 {
        out.push(b'\n');
    }
    out.extend_from_slice(header("obj.o/", 4).as_bytes());
    out.extend_from_slice(b"\0\0\0\0");
    out
}

fn bundle_fixtures(h: &mut Harness) {
    let cases: Vec<BundleCase> = vec![
        (
            "clean",
            vec![
                (
                    "usr/bin/papyrine",
                    elf_with_needed(&["libc.so.6", "libm.so.6", "libwebkit2gtk-4.1.so.0"]),
                ),
                ("usr/share/readme.txt", b"hi".to_vec()),
            ],
            false,
        ),
        (
            "unlisted-linked-lib",
            vec![(
                "usr/bin/papyrine",
                elf_with_needed(&["libc.so.6", "libmystery.so.1"]),
            )],
            true,
        ),
        (
            "linked-openssl",
            vec![(
                "usr/bin/papyrine",
                elf_with_needed(&["libc.so.6", "libssl.so.3"]),
            )],
            true,
        ),
        (
            "banned-file-gnutls",
            vec![("usr/lib/libgnutls.so.30", vec![0u8; 16])],
            true,
        ),
        (
            "banned-file-poppler",
            vec![("usr/lib/libpoppler.so.140", vec![0u8; 16])],
            true,
        ),
        (
            "banned-file-mupdf",
            vec![("Contents/Frameworks/libmupdf.dylib", vec![0u8; 16])],
            true,
        ),
        (
            "unlisted-bundled-lib",
            vec![("usr/lib/libsurprise.so.1", vec![0u8; 16])],
            true,
        ),
        (
            "static-archive-gnutls-symbols",
            vec![("usr/lib/libx.a", ar_with_symbols(&["foo", "gnutls_init"]))],
            true,
        ),
        (
            "static-archive-openssl-symbols",
            vec![("usr/lib/libx.a", ar_with_symbols(&["SSL_new"]))],
            true,
        ),
        (
            "static-archive-clean",
            vec![("usr/lib/libx.a", ar_with_symbols(&["foo", "bar"]))],
            false,
        ),
    ];
    for (name, files, fail) in cases {
        let d = h.dir(&format!("bundle-{name}"));
        for (p, c) in files {
            write(&d.join(p), c);
        }
        let out = h.run(
            "inspect-bundle",
            &[&s(&d), "--manifest", &s(&d.join("none.toml"))],
        );
        h.record(&format!("inspect-bundle: {name}"), fail, &out);
    }
}

fn main() {
    let require_all = std::env::args().any(|a| a == "--require-all");
    let root = repo_root(&std::env::current_dir().unwrap());
    let exe_path = std::env::current_exe().unwrap();
    let tmp = std::env::temp_dir().join(format!("gates-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let mut h = Harness {
        root,
        tmp: tmp.clone(),
        bin_dir: exe_path.parent().unwrap().to_path_buf(),
        passed: 0,
        failed: 0,
        skipped: 0,
    };
    cargo_deny_fixtures(&mut h);
    native_fixtures(&mut h);
    js_fixtures(&mut h);
    qpdf_fixtures(&mut h);
    corpus_fixtures(&mut h);
    notices_fixtures(&mut h);
    budget_fixtures(&mut h);
    bundle_fixtures(&mut h);
    let _ = fs::remove_dir_all(&tmp);
    println!(
        "gates-selftest: {} ok, {} failed, {} skipped",
        h.passed, h.failed, h.skipped
    );
    if h.failed > 0 || (require_all && h.skipped > 0) {
        std::process::exit(1);
    }
}
