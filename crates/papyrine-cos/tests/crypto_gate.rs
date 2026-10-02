//! ADR-021 gate: qpdf must be built with native crypto only.

use std::process::Command;

#[test]
fn registered_crypto_impls_is_exactly_native() {
    assert_eq!(
        papyrine_cos::registered_crypto_impls().unwrap(),
        vec!["native".to_string()]
    );
    assert_eq!(papyrine_cos::default_crypto_impl().unwrap(), "native");
}

#[test]
fn cmake_configuration_is_native_only() {
    let cfg = qpdf_sys::CRYPTO_BUILD_CONFIG;
    for line in [
        "USE_IMPLICIT_CRYPTO:BOOL=OFF",
        "REQUIRE_CRYPTO_NATIVE:BOOL=ON",
        "REQUIRE_CRYPTO_OPENSSL:BOOL=OFF",
        "REQUIRE_CRYPTO_GNUTLS:BOOL=OFF",
        "DEFAULT_CRYPTO:STRING=native",
    ] {
        assert!(cfg.contains(line), "missing {line} in:\n{cfg}");
    }
    assert!(cfg.contains("QPDFCrypto_native"), "{cfg}");
    assert!(!cfg.contains("openssl") && !cfg.contains("gnutls"), "{cfg}");
}

// Symbol-name prefixes the bundle gate also scans for (ARCHITECTURE section 12).
const FORBIDDEN_SYMBOL_PREFIXES: &[&str] =
    &["gnutls_", "EVP_", "OPENSSL_", "SSL_", "CRYPTO_", "ERR_"];
const FORBIDDEN_LIBS: &[&str] = &["libssl", "libcrypto", "libgnutls"];

#[test]
fn linked_binary_has_no_openssl_or_gnutls() {
    if !(cfg!(target_os = "macos") || cfg!(target_os = "linux")) {
        eprintln!("skipped: nm/otool/ldd checks are macOS and Linux only");
        return;
    }
    let exe = std::env::current_exe().unwrap();

    // Symbol table.
    let nm = match Command::new("nm").arg("-a").arg(&exe).output() {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!("skipped: nm not available");
            return;
        }
    };
    let syms = String::from_utf8_lossy(&nm.stdout);
    assert!(
        syms.contains("papyrine"),
        "nm output looks empty (stripped binary?)"
    );
    // Sanity: native crypto symbols are present, so the check below is meaningful.
    assert!(
        syms.contains("QPDFCrypto_native"),
        "native provider not found in binary"
    );
    let hits: Vec<&str> = syms
        .lines()
        .filter_map(|l| l.split_whitespace().last())
        .map(|name| name.strip_prefix('_').unwrap_or(name))
        .filter(|name| {
            FORBIDDEN_SYMBOL_PREFIXES
                .iter()
                .any(|p| name.starts_with(p))
        })
        .take(5)
        .collect();
    assert!(
        hits.is_empty(),
        "OpenSSL/GnuTLS-style symbols in binary: {hits:?}"
    );

    // Dynamic dependencies.
    let deps = if cfg!(target_os = "macos") {
        Command::new("otool").arg("-L").arg(&exe).output()
    } else {
        Command::new("ldd").arg(&exe).output()
    };
    if let Ok(o) = deps {
        let text = String::from_utf8_lossy(&o.stdout);
        for lib in FORBIDDEN_LIBS {
            assert!(!text.contains(lib), "binary links {lib}:\n{text}");
        }
    }
}
