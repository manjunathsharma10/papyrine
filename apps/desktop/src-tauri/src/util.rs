//! Small helpers shared by the broker modules.

use std::path::Path;
use std::time::SystemTime;

use papyrine_ipc::{DocSource, Handle, SharedRegion};

use crate::error::{HostErr, Result};

/// A second mapping of the same pages, for sending to a child while keeping ours.
pub fn dup_region(r: &SharedRegion) -> Result<SharedRegion> {
    let h = r.try_clone_handle()?;
    Ok(SharedRegion::from_handle(h, r.len())?)
}

/// A snapshot section the host owns (and the renderer maps read-only).
pub struct Section {
    pub region: SharedRegion,
    pub len: u64,
}

impl Section {
    pub fn source(&self) -> Result<DocSource> {
        Ok(DocSource::Shared {
            region: dup_region(&self.region)?,
            len: self.len,
        })
    }
}

/// The document's file as handed to children: an open read-only handle.
pub struct BaseFile {
    file: std::fs::File,
    pub len: u64,
}

impl BaseFile {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }

    pub fn source(&self) -> Result<DocSource> {
        let f = self.file.try_clone()?;
        Ok(DocSource::File {
            handle: Handle::from_file(f),
            len: self.len,
        })
    }

    /// Positional read (the handle is shared with children, so no cursor is used).
    pub fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, off)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, off)
        }
    }

    /// Copy the first `len` bytes into `out`; `Err` if the file is shorter.
    pub fn copy_prefix(&self, len: u64, out: &mut impl std::io::Write) -> std::io::Result<()> {
        let mut buf = vec![0u8; 1 << 20];
        let mut off = 0u64;
        while off < len {
            let want = buf.len().min((len - off) as usize);
            let n = self.read_at(off, &mut buf[..want])?;
            if n == 0 {
                return Err(std::io::Error::other(
                    "the document's file is shorter than the engine expects",
                ));
            }
            out.write_all(&buf[..n])?;
            off += n as u64;
        }
        Ok(())
    }

    /// "%PDF-1.7" header version, if present in the first KiB.
    pub fn pdf_version(&self) -> String {
        let mut buf = [0u8; 1024];
        let n = self.read_at(0, &mut buf).unwrap_or(0);
        let hay = &buf[..n];
        hay.windows(5)
            .position(|w| w == b"%PDF-")
            .map(|i| {
                hay[i + 5..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit() || **b == b'.')
                    .map(|b| *b as char)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// (mtime ms, size) of a file, for external-change detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub mtime_ms: u64,
    pub size: u64,
}

impl FileStat {
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        let mtime_ms = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Some(Self {
            mtime_ms,
            size: m.len(),
        })
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Random lowercase hex id of `bytes` bytes.
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if getrandom::fill(&mut b).is_err() {
        // Extremely unlikely; fall back to time + pid so ids stay unique per process.
        let t = now_ms().to_le_bytes();
        for (i, x) in b.iter_mut().enumerate() {
            *x = t[i % 8] ^ (std::process::id() as u8).wrapping_add(i as u8);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Size a shared output region for a PDF of about `base` bytes plus `extra`.
pub fn out_size(base: u64, extra: u64) -> usize {
    (base + extra + (4 << 20)).min(usize::MAX as u64 / 2) as usize
}

pub fn parse_needed(message: &str) -> Option<usize> {
    message
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

/// `p` canonicalised, if it is inside `dir` (the engine is untrusted: it may only hand the
/// host files from its own temp directory).
pub fn within(dir: &Path, p: &Path) -> Option<std::path::PathBuf> {
    let d = std::fs::canonicalize(dir).ok()?;
    let c = std::fs::canonicalize(p).ok()?;
    (c.starts_with(&d) && c.is_file()).then_some(c)
}

/// PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`, every part after the year optional) as ISO 8601.
/// Anything unparsable is returned empty rather than guessed.
pub fn pdf_date_to_iso(d: &str) -> String {
    let d = d.trim();
    let d = d.strip_prefix("D:").unwrap_or(d);
    let digits =
        |from: usize, len: usize| -> Option<u32> { d.get(from..from + len)?.parse::<u32>().ok() };
    let Some(year) = digits(0, 4) else {
        return String::new();
    };
    let month = digits(4, 2).unwrap_or(1).clamp(1, 12);
    let day = digits(6, 2).unwrap_or(1).clamp(1, 31);
    let (h, m, s) = (
        digits(8, 2).unwrap_or(0),
        digits(10, 2).unwrap_or(0),
        digits(12, 2).unwrap_or(0),
    );
    if h > 23 || m > 59 || s > 60 {
        return String::new();
    }
    let rest = d.get(14..).unwrap_or("");
    let tz = match rest.chars().next() {
        Some('Z') | None => "Z".to_string(),
        Some(c @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            let hh = t.get(0..2).unwrap_or("00");
            let mm = t.get(2..4).unwrap_or("00");
            format!("{c}{hh}:{mm}")
        }
        _ => "Z".to_string(),
    };
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}{tz}")
}

/// Unix milliseconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
pub fn iso_from_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn invalid(m: impl Into<String>) -> HostErr {
    HostErr::internal(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_dates_become_iso() {
        assert_eq!(
            pdf_date_to_iso("D:20240131120000+01'00'"),
            "2024-01-31T12:00:00+01:00"
        );
        assert_eq!(pdf_date_to_iso("D:20240131120000Z"), "2024-01-31T12:00:00Z");
        assert_eq!(pdf_date_to_iso("D:2024"), "2024-01-01T00:00:00Z");
        assert_eq!(
            pdf_date_to_iso("D:20240131093015-0530"),
            "2024-01-31T09:30:15-05:30"
        );
        assert_eq!(pdf_date_to_iso("garbage"), "");
        assert_eq!(pdf_date_to_iso("D:20241301250000"), "");
    }

    #[test]
    fn unix_ms_to_iso() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_ms(1_709_210_096_000), "2024-02-29T12:34:56Z");
        assert_eq!(iso_from_ms(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn engine_output_must_stay_in_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("a.tmp");
        std::fs::write(&inside, b"x").unwrap();
        assert!(within(dir.path(), &inside).is_some());
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("b.tmp");
        std::fs::write(&outside, b"x").unwrap();
        assert!(within(dir.path(), &outside).is_none());
        let sneaky = dir
            .path()
            .join("..")
            .join(other.path().file_name().unwrap())
            .join("b.tmp");
        assert!(within(dir.path(), &sneaky).is_none());
        assert!(
            within(dir.path(), dir.path()).is_none(),
            "directories are not files"
        );
    }

    #[test]
    fn needed_size_is_parsed_from_the_message() {
        assert_eq!(
            parse_needed("output region too small, need 123456 bytes"),
            Some(123456)
        );
        assert_eq!(parse_needed("nothing"), None);
    }
}
