//! Builds zlib, libjpeg-turbo and qpdf (native crypto only) from the pinned sources fetched by
//! `third_party/fetch`, then compiles the cxx shim. No network access, no system qpdf/openssl/gnutls.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

struct Lib {
    dir: PathBuf,
}

fn load_lib(root: &Path, name: &str) -> Lib {
    let toml_path = root.join("third_party/native.toml");
    println!("cargo:rerun-if-changed={}", toml_path.display());
    let text = fs::read_to_string(&toml_path).expect("read third_party/native.toml");
    let doc: toml::Table = text.parse().expect("parse native.toml");
    let libs = doc["library"].as_array().expect("[[library]] entries");
    let entry = libs
        .iter()
        .find(|l| l.get("name").and_then(|n| n.as_str()) == Some(name))
        .unwrap_or_else(|| panic!("{name} missing from third_party/native.toml"));
    let version = entry["version"].as_str().unwrap();
    let sha = entry["sha256"].as_str().unwrap();
    let dir = root.join(format!("third_party/cache/src/{name}-{version}"));
    let stamp = dir.join(".sha256");
    let ok = fs::read_to_string(&stamp)
        .map(|s| s.trim() == sha)
        .unwrap_or(false);
    if !ok {
        panic!(
            "\n\n{name} {version} sources are missing or stale in {}.\n\
             Run `third_party/fetch` from the repository root, then rebuild.\n\
             (The build never downloads anything itself.)\n",
            dir.display()
        );
    }
    Lib { dir }
}

fn collect_objects(dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_objects(&p, out);
        } else if let Some(n) = p.file_name().and_then(|n| n.to_str())
            && (n.ends_with(".o") || n.ends_with(".obj"))
        {
            out.push(n.to_string());
        }
    }
}

/// `canonicalize` yields `\\?\C:\...` verbatim paths on Windows; CMake/MSBuild mangle them into
/// `\\\(1,1): error C1083`. Strip the prefix when the remainder is an ordinary drive path.
fn simplify(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 3 && b[1] == b':' && b[2] == b'\\' {
            return PathBuf::from(rest);
        }
    }
    p
}

fn on_off(b: bool) -> &'static str {
    if b { "ON" } else { "OFF" }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = simplify(manifest.join("../..").canonicalize().unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=shim");

    let zlib = load_lib(&root, "zlib");
    let jpeg = load_lib(&root, "libjpeg-turbo");
    let qpdf = load_lib(&root, "qpdf");

    // Third-party code always builds optimised: debug qpdf is needlessly slow for tests.
    let profile = "Release";
    // Never let CMake find Homebrew/system copies of zlib, jpeg, openssl or gnutls.
    let ignore_prefixes = "/opt/homebrew;/usr/local;/opt/local";

    // zlib
    let zlib_prefix = out.join("zlib-install");
    cmake::Config::new(&zlib.dir)
        .out_dir(out.join("zlib-build"))
        .profile(profile)
        .define("CMAKE_INSTALL_PREFIX", &zlib_prefix)
        .define("ZLIB_BUILD_SHARED", "OFF")
        .define("ZLIB_BUILD_STATIC", "ON")
        .define("ZLIB_BUILD_TESTING", "OFF")
        .define("ZLIB_BUILD_EXAMPLES", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_IGNORE_PREFIX_PATH", ignore_prefixes)
        .build();

    // libjpeg-turbo: SIMD off (portable, no NASM requirement); qpdf only uses it for DCT image
    // re-encoding, which is not a hot path. TurboJPEG API, tools and tests are not needed.
    let jpeg_prefix = out.join("jpeg-install");
    cmake::Config::new(&jpeg.dir)
        .out_dir(out.join("jpeg-build"))
        .profile(profile)
        .define("CMAKE_INSTALL_PREFIX", &jpeg_prefix)
        .define("ENABLE_SHARED", "0")
        .define("ENABLE_STATIC", "1")
        .define("WITH_SIMD", "0")
        .define("WITH_TURBOJPEG", "0")
        .define("WITH_TOOLS", "0")
        .define("WITH_TESTS", "0")
        .define("WITH_FUZZ", "0")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .define("CMAKE_IGNORE_PREFIX_PATH", ignore_prefixes)
        .build();

    // qpdf locates zlib and libjpeg through pkg-config first. Our install prefixes contain spaces
    // on some machines, which .pc files cannot express, so point pkg-config at an empty directory
    // (never at Homebrew/system) and let qpdf fall back to find_path/find_library, which search
    // CMAKE_PREFIX_PATH first.
    let empty_pc = out.join("empty-pkgconfig");
    fs::create_dir_all(&empty_pc).unwrap();
    let prefix_path = format!("{};{}", zlib_prefix.display(), jpeg_prefix.display());

    let qpdf_build_dir = out.join("qpdf-build");
    cmake::Config::new(&qpdf.dir)
        .out_dir(&qpdf_build_dir)
        .profile(profile)
        .build_target("libqpdf")
        .env("PKG_CONFIG_LIBDIR", &empty_pc)
        .env("PKG_CONFIG_PATH", &empty_pc)
        .define("CMAKE_PREFIX_PATH", &prefix_path)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_STATIC_LIBS", "ON")
        .define("STATIC_JPEG", "ON")
        .define("BUILD_DOC", "OFF")
        .define("INSTALL_EXAMPLES", "OFF")
        .define("INSTALL_MANUAL", "OFF")
        .define("INSTALL_PKGCONFIG", "OFF")
        .define("INSTALL_CMAKE_PACKAGE", "OFF")
        .define("GENERATE_AUTO_JOB", "OFF")
        .define("ENABLE_QTC", "OFF")
        .define("REQUIRE_SHELLS", "OFF")
        .define("SKIP_OS_SECURE_RANDOM", "OFF")
        .define("USE_INSECURE_RANDOM", "OFF")
        .define("WERROR", "OFF")
        // Crypto: native only (ADR-021).
        .define("USE_IMPLICIT_CRYPTO", on_off(false))
        .define("REQUIRE_CRYPTO_NATIVE", on_off(true))
        .define("REQUIRE_CRYPTO_OPENSSL", on_off(false))
        .define("REQUIRE_CRYPTO_GNUTLS", on_off(false))
        .define("DEFAULT_CRYPTO", "native")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_IGNORE_PREFIX_PATH", ignore_prefixes)
        .build();

    let qpdf_lib_dir = qpdf_build_dir.join("build/libqpdf");
    assert!(
        qpdf_lib_dir.join("libqpdf.a").exists() || qpdf_lib_dir.join("qpdf.lib").exists(),
        "libqpdf static archive not found in {}",
        qpdf_lib_dir.display()
    );

    // Record the effective CMake configuration for the license gate (ARCHITECTURE section 12):
    // the cache entries plus the crypto object files that were actually compiled.
    let cache = qpdf_build_dir.join("build/CMakeCache.txt");
    let text = fs::read_to_string(&cache).expect("read qpdf CMakeCache.txt");
    let mut gate = String::new();
    for key in [
        "USE_IMPLICIT_CRYPTO",
        "REQUIRE_CRYPTO_NATIVE",
        "REQUIRE_CRYPTO_OPENSSL",
        "REQUIRE_CRYPTO_GNUTLS",
        "DEFAULT_CRYPTO",
    ] {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("{key}:")))
            .unwrap_or_else(|| panic!("{key} not in CMakeCache"));
        gate.push_str(line);
        gate.push('\n');
    }
    let mut objs = Vec::new();
    collect_objects(&qpdf_build_dir, &mut objs);
    objs.sort();
    let crypto_objs: Vec<_> = objs
        .iter()
        .filter(|o| o.contains("_native") || o.contains("_openssl") || o.contains("_gnutls"))
        .cloned()
        .collect();
    for o in &crypto_objs {
        assert!(
            !o.contains("openssl") && !o.contains("gnutls"),
            "qpdf compiled a forbidden crypto provider: {o}"
        );
        gate.push_str(&format!("object: {o}\n"));
    }
    assert!(
        !crypto_objs.is_empty(),
        "qpdf native crypto objects not found"
    );
    fs::write(out.join("qpdf-crypto-config.txt"), gate).unwrap();

    println!("cargo:rustc-link-search=native={}", qpdf_lib_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        zlib_prefix.join("lib").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        jpeg_prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=qpdf");
    println!("cargo:rustc-link-lib=static=jpeg");
    let zname = if target.contains("windows-msvc") {
        "zlibstatic"
    } else {
        "z"
    };
    println!("cargo:rustc-link-lib=static={zname}");
    if target.contains("apple") || target.contains("freebsd") {
        println!("cargo:rustc-link-lib=c++");
    } else if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=stdc++");
    }

    let mut b = cxx_build::bridge("src/lib.rs");
    b.file("shim/shim.cc")
        .file("shim/helpers.cc")
        .include("shim")
        .include(qpdf.dir.join("include"))
        .include(qpdf_lib_dir.clone())
        .std("c++20")
        .warnings(false);
    if target.contains("msvc") {
        b.flag("/EHsc");
    }
    b.compile("papyrine_qpdf_shim");
}
