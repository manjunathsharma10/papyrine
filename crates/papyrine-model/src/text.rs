//! PDF text strings (PDFDocEncoding, UTF-16BE/LE with BOM, UTF-8 with BOM) and PDF dates.

/// PDFDocEncoding 0x18..=0x1F.
const LOW: [char; 8] = [
    '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}',
];
/// PDFDocEncoding 0x80..=0xA0 (0x9F is undefined and maps to U+FFFD).
const HIGH: [char; 33] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}',
    '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}',
    '\u{20AC}',
];

fn pdfdoc_char(b: u8) -> char {
    match b {
        0x18..=0x1F => LOW[usize::from(b - 0x18)],
        0x80..=0xA0 => HIGH[usize::from(b - 0x80)],
        0xAD => '\u{FFFD}',
        _ => char::from(b),
    }
}

/// Decode PDFDocEncoding.
pub fn decode_pdfdoc(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| pdfdoc_char(b)).collect()
}

fn decode_utf16(bytes: &[u8], little: bool) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|c| {
        if little {
            u16::from_le_bytes([c[0], c[1]])
        } else {
            u16::from_be_bytes([c[0], c[1]])
        }
    });
    let s: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    strip_language_escapes(&s)
}

/// PDF 1.7 allows `ESC <lang> ESC` markers inside Unicode text strings.
fn strip_language_escapes(s: &str) -> String {
    if !s.contains('\u{1B}') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut in_esc = false;
    for c in s.chars() {
        if c == '\u{1B}' {
            in_esc = !in_esc;
        } else if !in_esc {
            out.push(c);
        }
    }
    out
}

/// Decode a PDF text string (section 7.9.2.2): UTF-16BE/LE or UTF-8 with a BOM, else PDFDocEncoding.
pub fn decode_text_string(bytes: &[u8]) -> String {
    match bytes {
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, false),
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => strip_language_escapes(&String::from_utf8_lossy(rest)),
        _ => decode_pdfdoc(bytes),
    }
}

/// Decode script or other "text stream" bytes: a BOM wins, otherwise valid UTF-8, otherwise
/// PDFDocEncoding. Real files often store UTF-8 in JavaScript without a BOM.
pub fn decode_text_lenient(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xEF, 0xBB, 0xBF])
    {
        return decode_text_string(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => decode_pdfdoc(bytes),
    }
}

/// Encode as a PDF text string: PDFDocEncoding when every character fits, else UTF-16BE with BOM.
pub fn encode_text_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut ok = true;
    for c in s.chars() {
        let b = (0u16..=255).find(|&b| {
            let b = b as u8;
            let cc = pdfdoc_char(b);
            cc == c && cc != '\u{FFFD}'
        });
        match b {
            Some(b) => out.push(b as u8),
            None => {
                ok = false;
                break;
            }
        }
    }
    if ok {
        return out;
    }
    let mut out = vec![0xFE, 0xFF];
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_be_bytes());
    }
    out
}

/// A PDF date (section 7.9.4). Missing trailing components take their defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Offset from UTC in minutes; `None` when the date carries no zone (unknown local time).
    pub utc_offset_minutes: Option<i16>,
}

impl PdfDate {
    /// Parse `D:YYYYMMDDHHmmSSOHH'mm'`. The `D:` prefix, everything after the year, and the
    /// closing apostrophe are optional; some writers use `HH:mm` or `HHmm`. Out-of-range fields
    /// make the parse fail.
    pub fn parse(s: &str) -> Option<PdfDate> {
        let s = s.trim();
        let s = s.strip_prefix("D:").unwrap_or(s);
        let b = s.as_bytes();
        let digits = |from: usize, n: usize| -> Option<u32> {
            let part = b.get(from..from + n)?;
            if !part.iter().all(u8::is_ascii_digit) {
                return None;
            }
            std::str::from_utf8(part).ok()?.parse().ok()
        };
        let year = digits(0, 4)? as i32;
        let mut pos = 4;
        let mut field = |default: u32, max: u32, min: u32| -> Option<u32> {
            match digits(pos, 2) {
                Some(v) if v >= min && v <= max => {
                    pos += 2;
                    Some(v)
                }
                Some(_) => None,
                None => Some(default),
            }
        };
        let month = field(1, 12, 1)?;
        let day = field(1, 31, 1)?;
        let hour = field(0, 23, 0)?;
        let minute = field(0, 59, 0)?;
        let second = field(0, 60, 0)?; // leap second tolerated
        let rest = &s[pos.min(s.len())..];
        let utc_offset_minutes = match rest.as_bytes().first() {
            None => None,
            Some(b'Z') | Some(b'z') => Some(0),
            Some(&sign @ (b'+' | b'-')) => {
                let digits_of: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
                let (hh, mm) = match digits_of.len() {
                    0 => (0, 0),
                    1 | 2 => (digits_of.parse::<i16>().ok()?, 0),
                    _ => (
                        digits_of[..2].parse::<i16>().ok()?,
                        digits_of[2..4.min(digits_of.len())]
                            .parse::<i16>()
                            .unwrap_or(0),
                    ),
                };
                if hh > 23 || mm > 59 {
                    return None;
                }
                let m = hh * 60 + mm;
                Some(if sign == b'-' { -m } else { m })
            }
            _ => None,
        };
        let date = PdfDate {
            year,
            month: month as u8,
            day: day as u8,
            hour: hour as u8,
            minute: minute as u8,
            second: second as u8,
            utc_offset_minutes,
        };
        (date.day <= days_in_month(year, date.month)).then_some(date)
    }

    /// Seconds since the Unix epoch, treating a missing zone as UTC.
    pub fn to_unix(&self) -> i64 {
        let days = days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        );
        let local = days * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        local - i64::from(self.utc_offset_minutes.unwrap_or(0)) * 60
    }

    /// `YYYY-MM-DDTHH:MM:SS` plus `Z` or `+HH:MM` when a zone is known.
    pub fn to_iso8601(&self) -> String {
        let mut s = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        match self.utc_offset_minutes {
            None => {}
            Some(0) => s.push('Z'),
            Some(m) => {
                let (sign, m) = if m < 0 { ('-', -m) } else { ('+', m) };
                s.push_str(&format!("{sign}{:02}:{:02}", m / 60, m % 60));
            }
        }
        s
    }
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfdoc_specials() {
        assert_eq!(
            decode_text_string(b"a\x80b\xa0\x9c"),
            "a\u{2022}b\u{20ac}\u{0153}"
        );
        assert_eq!(decode_text_string(b"caf\xe9"), "caf\u{e9}");
        assert_eq!(decode_text_string(b"\x18\x1f"), "\u{2d8}\u{2dc}");
    }

    #[test]
    fn utf16_and_utf8() {
        assert_eq!(
            decode_text_string(b"\xfe\xff\x00H\x00i\x26\x3a"),
            "Hi\u{263a}"
        );
        assert_eq!(decode_text_string(b"\xff\xfeH\x00i\x00"), "Hi");
        assert_eq!(decode_text_string(b"\xef\xbb\xbfh\xc3\xa9"), "h\u{e9}");
        // surrogate pair and an unpaired surrogate
        assert_eq!(decode_text_string(b"\xfe\xff\xd8\x3d\xde\x00"), "\u{1F600}");
        assert_eq!(decode_text_string(b"\xfe\xff\xd8\x3d"), "\u{FFFD}");
        // language escape
        assert_eq!(
            decode_text_string(b"\xfe\xff\x00\x1b\x00e\x00n\x00\x1b\x00H\x00i"),
            "Hi"
        );
        // odd trailing byte is ignored
        assert_eq!(decode_text_string(b"\xfe\xff\x00A\x00"), "A");
    }

    #[test]
    fn encode_roundtrip() {
        for s in [
            "plain",
            "caf\u{e9} \u{2022}",
            "\u{4e2d}\u{6587}",
            "\u{1F600}x",
            "",
        ] {
            assert_eq!(decode_text_string(&encode_text_string(s)), s);
        }
        assert_eq!(encode_text_string("abc"), b"abc");
        assert_eq!(&encode_text_string("\u{4e2d}")[..2], &[0xFE, 0xFF]);
    }

    #[test]
    fn dates() {
        let d = PdfDate::parse("D:20240229123456+05'30'").unwrap();
        assert_eq!(
            (d.year, d.month, d.day, d.hour, d.minute, d.second),
            (2024, 2, 29, 12, 34, 56)
        );
        assert_eq!(d.utc_offset_minutes, Some(330));
        assert_eq!(d.to_iso8601(), "2024-02-29T12:34:56+05:30");
        assert_eq!(
            PdfDate::parse("D:20240229123456Z").unwrap().to_unix(),
            1_709_210_096
        );
        assert_eq!(
            PdfDate::parse("D:20240229123456-08'00'")
                .unwrap()
                .utc_offset_minutes,
            Some(-480)
        );
        assert_eq!(
            PdfDate::parse("D:1999").unwrap().to_iso8601(),
            "1999-01-01T00:00:00"
        );
        assert_eq!(PdfDate::parse("D:199912").unwrap().month, 12);
        assert_eq!(
            PdfDate::parse("20000101000000Z").unwrap().to_unix(),
            946_684_800
        );
        assert_eq!(
            PdfDate::parse("D:20240101120000+0100")
                .unwrap()
                .utc_offset_minutes,
            Some(60)
        );
        assert_eq!(
            PdfDate::parse("D:20240101120000+01'")
                .unwrap()
                .utc_offset_minutes,
            Some(60)
        );
        assert!(PdfDate::parse("D:20230229").is_none());
        assert!(PdfDate::parse("D:20241301").is_none());
        assert!(PdfDate::parse("garbage").is_none());
        assert!(PdfDate::parse("").is_none());
        assert_eq!(PdfDate::parse("D:19700101000000Z").unwrap().to_unix(), 0);
        assert_eq!(PdfDate::parse("D:19691231235959Z").unwrap().to_unix(), -1);
    }
}
