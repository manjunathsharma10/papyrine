//! Verified download (no silent install). The only trusted facts are the
//! hash and size taken from a signature-verified `updates.json`.

use crate::transport::{Request, Transport};
use crate::updates::Download;
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_INSTALLER_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDownload {
    pub path: PathBuf,
    pub sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn safe_file_name(url: &str) -> String {
    let last = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let clean = clean.trim_start_matches('.');
    if clean.is_empty() {
        "papyrine-installer".into()
    } else {
        clean.to_string()
    }
}

/// Download `d` into `dest_dir`, checking size and SHA-256 while streaming.
/// On any mismatch the partial file is deleted and an error is returned.
pub fn download_verified(
    transport: &dyn Transport,
    d: &Download,
    dest_dir: &Path,
) -> Result<VerifiedDownload> {
    if !d.url.starts_with("https://") {
        return Err(Error::Integrity("download URL is not https".into()));
    }
    if d.size == 0 || d.size > MAX_INSTALLER_BYTES {
        return Err(Error::Integrity("declared size out of range".into()));
    }
    let want = d.sha256.to_ascii_lowercase();
    if want.len() != 64 || !want.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Integrity("declared hash is not SHA-256 hex".into()));
    }
    let io = |e: std::io::Error| Error::Io(e.to_string());
    std::fs::create_dir_all(dest_dir).map_err(io)?;

    let resp = transport.get(&Request { url: d.url.clone() })?;
    if resp.status != 200 {
        return Err(Error::Io(format!("download: HTTP {}", resp.status)));
    }
    let name = safe_file_name(&d.url);
    let part = dest_dir.join(format!(".{name}.part"));
    let result = (|| -> Result<String> {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part)
            .map_err(io)?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut body = resp.body.take(d.size + 1);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = body.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > d.size {
                return Err(Error::Integrity("download is larger than declared".into()));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(io)?;
        }
        out.flush().map_err(io)?;
        out.sync_all().map_err(io)?;
        if total != d.size {
            return Err(Error::Integrity(format!(
                "download is {total} bytes, expected {}",
                d.size
            )));
        }
        let got = hex(&hasher.finalize());
        if got != want {
            return Err(Error::Integrity("SHA-256 mismatch".into()));
        }
        Ok(got)
    })();
    match result {
        Ok(sha256) => {
            let path = dest_dir.join(&name);
            std::fs::rename(&part, &path).map_err(io)?;
            Ok(VerifiedDownload { path, sha256 })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Hands the verified installer to the OS. The user finishes the install.
pub trait Opener {
    fn open(&self, path: &Path) -> Result<()>;
}

pub struct SystemOpener;

impl Opener for SystemOpener {
    fn open(&self, path: &Path) -> Result<()> {
        let (prog, pre): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("open", &[])
        } else if cfg!(windows) {
            ("explorer", &[])
        } else {
            ("xdg-open", &[])
        };
        std::process::Command::new(prog)
            .args(pre)
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| Error::Io(format!("cannot open installer: {e}")))
    }
}

/// Download, verify, then hand off to `opener`.
pub fn download_and_open(
    transport: &dyn Transport,
    d: &Download,
    dest_dir: &Path,
    opener: &dyn Opener,
) -> Result<VerifiedDownload> {
    let v = download_verified(transport, d, dest_dir)?;
    opener.open(&v.path)?;
    Ok(v)
}
