//! macOS: Seatbelt (`sandbox_init`) with a deny-by-default profile.
//!
//! `sandbox_init` is deprecated in the SDK headers but remains the supported
//! way for a process to confine itself (Chromium, Firefox and Safari helpers
//! use it). The profile is generated per child with real (canonical) paths.

use crate::{Error, Profile, Report, Result};
use std::ffi::{CStr, CString, c_char, c_int};
use std::fmt::Write;
use std::path::Path;

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// SBPL string literal with `\` and `"` escaped.
fn lit(p: &Path) -> String {
    let s = p.to_string_lossy();
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('"');
    o
}

/// The Seatbelt profile text for `p` (already normalised). Public to the crate
/// so tests can inspect it.
pub(crate) fn profile_text(p: &Profile) -> String {
    let mut s = String::new();
    s.push_str(
        r#"(version 1)
(deny default)
; Nothing is logged: a denied probe is expected behaviour, not an error to surface.
(deny network*)
(deny process-exec* process-fork)

; --- the process itself -----------------------------------------------------
(allow signal (target self))
(allow process-info-pidinfo (target self))
(allow process-info-setcontrol (target self))
(allow sysctl-read)
(allow system-info)
(allow iokit-get-properties)
(allow ipc-posix-shm-read-data ipc-posix-shm-read-metadata)

; --- OS libraries, frameworks and fonts (read only) --------------------------
(allow file-read*
  (subpath "/System")
  (subpath "/usr/lib")
  (subpath "/usr/share")
  (subpath "/Library/Fonts")
  (subpath "/private/var/db/dyld")
  (subpath "/private/etc/ssl")
  (literal "/private/etc/localtime")
  (literal "/private/var/select/sandbox")
  (literal "/dev/null") (literal "/dev/zero")
  (literal "/dev/random") (literal "/dev/urandom"))
(allow file-write-data (literal "/dev/null"))

; --- metadata (stat, realpath) for the directories leading to allowed paths ---
(allow file-read-metadata (literal "/") (literal "/private") (literal "/private/var")
  (literal "/private/tmp") (literal "/var") (literal "/tmp") (literal "/etc")
  (literal "/Library") (literal "/dev")
  (literal "/private/var/folders"))

; --- mach services PDFium (CoreText fonts), qpdf and the Rust runtime need ----
(allow mach-lookup
  (global-name "com.apple.system.logger")
  (global-name "com.apple.logd")
  (global-name "com.apple.diagnosticd")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.fonts")
  (global-name "com.apple.FontObjectsServer")
  (global-name "com.apple.fontservicesd")
  (global-name "com.apple.coreservices.launchservicesd")
  (global-name "com.apple.lsd.mapdb")
  (global-name "com.apple.SystemConfiguration.configd")
  (global-name "com.apple.cfprefsd.daemon")
  (global-name "com.apple.cfprefsd.agent")
  (global-name "com.apple.trustd.agent"))
"#,
    );

    // Parents of allowed trees: metadata only, so realpath()/stat() work
    // without exposing directory contents or file data.
    let mut parents: Vec<&Path> = Vec::new();
    let all = p
        .read_dirs
        .iter()
        .chain(p.read_files.iter())
        .chain(std::iter::once(&p.temp_dir));
    for path in all {
        for anc in path.ancestors().skip(1) {
            if !anc.as_os_str().is_empty() && !parents.contains(&anc) {
                parents.push(anc);
            }
        }
    }
    if !parents.is_empty() {
        s.push_str("(allow file-read-metadata\n");
        for a in parents {
            let _ = writeln!(s, "  (literal {})", lit(a));
        }
        s.push_str(")\n");
    }

    for d in &p.read_dirs {
        let _ = writeln!(s, "(allow file-read* (subpath {}))", lit(d));
    }
    for f in &p.read_files {
        let _ = writeln!(s, "(allow file-read* (literal {}))", lit(f));
    }
    let _ = writeln!(
        s,
        "(allow file-read* file-write* (subpath {}))",
        lit(&p.temp_dir)
    );
    s
}

pub(crate) fn apply(p: &Profile) -> Result<Report> {
    let text = profile_text(p);
    let c = CString::new(text).map_err(|_| Error::Setup("NUL in profile".into()))?;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: `c` is a valid NUL-terminated string; `err` receives a malloc'd
    // message that we free with sandbox_free_error. flags = 0 means the string
    // is a raw SBPL profile.
    let rc = unsafe { sandbox_init(c.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            // SAFETY: sandbox_init set a valid C string.
            let m = unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
            // SAFETY: allocated by sandbox_init.
            unsafe { sandbox_free_error(err) };
            m
        };
        return Err(Error::Setup(format!("sandbox_init: {msg}")));
    }
    Ok(Report {
        mechanisms: vec!["seatbelt".into()],
        degraded: vec![],
    })
}
