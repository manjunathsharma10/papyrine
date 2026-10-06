//! Small helpers: blob substitution in command parameters, output delivery, base64.

use std::io::Write;
use std::path::{Path, PathBuf};

use papyrine_ipc::{BlobRef, MappedFile};
use serde_json::Value;

use crate::proto::{Delivery, EngineError, INLINE_LIMIT, OutputFile, Result};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Bytes of a blob argument.
pub fn blob_bytes(b: &BlobRef) -> Result<Vec<u8>> {
    match b {
        BlobRef::Inline(v) => Ok(v.clone()),
        BlobRef::Shared {
            region,
            offset,
            len,
        } => {
            let (o, l) = (*offset as usize, *len as usize);
            if o.checked_add(l).is_none_or(|e| e > region.len()) {
                return Err(EngineError::Invalid("blob outside its region".into()));
            }
            Ok(region.to_vec(o, l))
        }
        BlobRef::File { handle, len } => {
            let m = MappedFile::map(handle, *len)?;
            Ok(<MappedFile as AsRef<[u8]>>::as_ref(&m).to_vec())
        }
    }
}

/// Replace every `{"$blob": i}` object in `params` with the base64 text of blob `i`, so commands
/// receive external inputs (an image, another PDF) as an ordinary JSON string field. The host
/// journals the original parameters plus the blob content, so replay never re-reads files.
pub fn substitute_blobs(params: &mut Value, blobs: &[BlobRef]) -> Result<()> {
    match params {
        Value::Object(m) => {
            if m.len() == 1
                && let Some(i) = m.get("$blob").and_then(Value::as_u64)
            {
                let b = blobs
                    .get(i as usize)
                    .ok_or_else(|| EngineError::Invalid(format!("no blob {i}")))?;
                *params = Value::String(base64(&blob_bytes(b)?));
                return Ok(());
            }
            for v in m.values_mut() {
                substitute_blobs(v, blobs)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                substitute_blobs(v, blobs)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The engine's scratch directory (its sandbox temp dir).
pub fn scratch_dir() -> PathBuf {
    std::env::temp_dir()
}

/// A unique file path in `dir`.
pub fn temp_path(dir: &Path, stem: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        "papyrine-engine-{}-{stem}-{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn write_temp(dir: &Path, stem: &str, bytes: &[u8]) -> Result<OutputFile> {
    let path = temp_path(dir, stem);
    let mut f = std::fs::File::create(&path)?;
    f.write_all(bytes)?;
    f.flush()?;
    Ok(OutputFile {
        path: path.to_string_lossy().into_owned(),
        len: bytes.len() as u64,
    })
}

/// Hand `bytes` to the host: into `region` when it fits, inline when small, else a temp file.
pub fn deliver(
    bytes: Vec<u8>,
    region: Option<&papyrine_ipc::SharedRegion>,
    dir: &Path,
    stem: &str,
) -> Result<Delivery> {
    if let Some(r) = region
        && bytes.len() <= r.len()
    {
        r.write_at(0, &bytes);
        return Ok(Delivery::Shared {
            len: bytes.len() as u64,
        });
    }
    if bytes.len() <= INLINE_LIMIT {
        return Ok(Delivery::Inline(bytes));
    }
    Ok(Delivery::File(write_temp(dir, stem, &bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn blob_markers_are_replaced() {
        let mut p = serde_json::json!({"a": {"$blob": 0}, "b": [{"$blob": 1}], "c": {"x": 1}});
        let blobs = [
            BlobRef::Inline(b"foo".to_vec()),
            BlobRef::Inline(b"f".to_vec()),
        ];
        substitute_blobs(&mut p, &blobs).unwrap();
        assert_eq!(p["a"], "Zm9v");
        assert_eq!(p["b"][0], "Zg==");
        assert_eq!(p["c"]["x"], 1);
        let mut bad = serde_json::json!({"$blob": 5});
        assert!(substitute_blobs(&mut bad, &blobs).is_err());
    }
}
