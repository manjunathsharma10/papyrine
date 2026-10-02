use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A user-visible string as a catalog key plus named arguments, so the UI layer localizes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizedText {
    pub key: String,
    pub args: BTreeMap<String, String>,
}

impl LocalizedText {
    pub fn new(key: impl Into<String>) -> Self {
        LocalizedText {
            key: key.into(),
            args: BTreeMap::new(),
        }
    }

    pub fn arg(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.args.insert(name.into(), value.to_string());
        self
    }
}

/// Encode a Rust string as a PDF text string: plain bytes when it is printable ASCII, UTF-16BE
/// with a byte-order mark otherwise.
pub fn encode_text_string(s: &str) -> Vec<u8> {
    if s.bytes()
        .all(|b| (0x20..0x7f).contains(&b) || matches!(b, b'\t' | b'\n' | b'\r'))
    {
        return s.as_bytes().to_vec();
    }
    let mut out = vec![0xFE, 0xFF];
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

/// Decode a PDF text string (UTF-16BE/LE with BOM, UTF-8 with BOM, else Latin-1-ish
/// PDFDocEncoding).
pub fn decode_text_string(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_string_round_trip() {
        for s in ["", "Hello", "Zażółć gęślą jaźń", "日本語 \u{1F600}"] {
            assert_eq!(decode_text_string(&encode_text_string(s)), s);
        }
        assert_eq!(encode_text_string("abc"), b"abc");
    }
}
