//! Tokenizer for content streams.

use crate::object::Object;
use crate::parser::{ParseError, ParseErrorKind};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Int(i64),
    /// Never integral; integral values are reported as `Int`.
    Real(f64),
    /// Decoded literal string `( ... )`.
    LiteralString(Vec<u8>),
    /// Decoded hex string `< ... >`.
    HexString(Vec<u8>),
    /// Decoded name (after `#xx` unescaping), without the slash.
    Name(Vec<u8>),
    ArrayStart,
    ArrayEnd,
    DictStart,
    DictEnd,
    /// Operator or bare word (`true`, `false`, `null` included); text is `span`.
    Keyword,
    /// Text after `%`, without the terminating EOL.
    Comment,
    /// A byte that cannot start any valid token: `)`, `>`, `{`, `}` or a
    /// non-printable byte outside strings and names.
    Stray(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

const WS: u8 = 1;
const DELIM: u8 = 2;

static CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    t[0] = WS;
    t[9] = WS;
    t[10] = WS;
    t[12] = WS;
    t[13] = WS;
    t[32] = WS;
    let d = b"()<>[]{}/%";
    let mut i = 0;
    while i < d.len() {
        t[d[i] as usize] = DELIM;
        i += 1;
    }
    t
};

#[inline]
pub fn is_whitespace(b: u8) -> bool {
    CLASS[b as usize] == WS
}

#[inline]
pub fn is_delimiter(b: u8) -> bool {
    CLASS[b as usize] == DELIM
}

#[inline]
pub fn is_regular(b: u8) -> bool {
    CLASS[b as usize] == 0
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Largest magnitude kept as `Int`; beyond it numbers are `Real`, so that
/// formatting an integral `Real` and re-reading it is stable.
const MAX_INT: f64 = 9.0e18;

pub(crate) fn number_from_f64(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() <= MAX_INT {
        Object::Int(v as i64)
    } else {
        Object::Real(v)
    }
}

pub struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
    errors: Vec<ParseError>,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            errors: Vec::new(),
        }
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    pub fn take_errors(&mut self) -> Vec<ParseError> {
        std::mem::take(&mut self.errors)
    }

    pub(crate) fn error(&mut self, offset: usize, kind: ParseErrorKind) {
        self.errors.push(ParseError { offset, kind });
    }

    fn skip_ws(&mut self) {
        while self.pos < self.data.len() && is_whitespace(self.data[self.pos]) {
            self.pos += 1;
        }
    }

    /// Next token, or `None` at end of input.
    pub fn next_token(&mut self) -> Option<Token> {
        self.skip_ws();
        let start = self.pos;
        let b = *self.data.get(start)?;
        let kind = match b {
            b'/' => {
                self.pos += 1;
                TokenKind::Name(self.scan_name())
            }
            b'(' => {
                self.pos += 1;
                TokenKind::LiteralString(self.scan_literal_string(start))
            }
            b'<' => {
                if self.data.get(start + 1) == Some(&b'<') {
                    self.pos += 2;
                    TokenKind::DictStart
                } else {
                    self.pos += 1;
                    TokenKind::HexString(self.scan_hex_string(start))
                }
            }
            b'>' => {
                if self.data.get(start + 1) == Some(&b'>') {
                    self.pos += 2;
                    TokenKind::DictEnd
                } else {
                    self.pos += 1;
                    self.error(start, ParseErrorKind::StrayDelimiter(b));
                    TokenKind::Stray(b)
                }
            }
            b'[' => {
                self.pos += 1;
                TokenKind::ArrayStart
            }
            b']' => {
                self.pos += 1;
                TokenKind::ArrayEnd
            }
            b'%' => {
                self.pos += 1;
                while self.pos < self.data.len() && !matches!(self.data[self.pos], b'\n' | b'\r') {
                    self.pos += 1;
                }
                TokenKind::Comment
            }
            b')' | b'{' | b'}' => {
                self.pos += 1;
                self.error(start, ParseErrorKind::StrayDelimiter(b));
                TokenKind::Stray(b)
            }
            0..=0x20 | 0x7F..=0xFF => {
                // Operators and numbers are printable ASCII; anything else is noise.
                self.pos += 1;
                self.error(start, ParseErrorKind::InvalidByte(b));
                TokenKind::Stray(b)
            }
            _ => {
                let mut end = start + 1;
                while end < self.data.len()
                    && is_regular(self.data[end])
                    && (0x21..0x7F).contains(&self.data[end])
                {
                    end += 1;
                }
                self.pos = end;
                match classify_number(&self.data[start..end]) {
                    Some(n) => match number_from_f64_or_int(n) {
                        Object::Int(i) => TokenKind::Int(i),
                        Object::Real(r) => TokenKind::Real(r),
                        _ => unreachable!(),
                    },
                    None => TokenKind::Keyword,
                }
            }
        };
        Some(Token {
            kind,
            span: start..self.pos,
        })
    }

    fn regular_run_end(&self, start: usize) -> usize {
        let mut i = start;
        while i < self.data.len() && is_regular(self.data[i]) {
            i += 1;
        }
        i
    }

    fn scan_name(&mut self) -> Vec<u8> {
        let end = self.regular_run_end(self.pos);
        let raw = &self.data[self.pos..end];
        self.pos = end;
        if !raw.contains(&b'#') {
            return raw.to_vec();
        }
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            if raw[i] == b'#'
                && i + 2 < raw.len()
                && let (Some(h), Some(l)) = (hex_val(raw[i + 1]), hex_val(raw[i + 2]))
            {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
            out.push(raw[i]);
            i += 1;
        }
        out
    }

    fn scan_literal_string(&mut self, start: usize) -> Vec<u8> {
        let d = self.data;
        let mut out = Vec::new();
        let mut depth = 1u32;
        let mut i = self.pos;
        while i < d.len() {
            let c = d[i];
            i += 1;
            match c {
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos = i;
                        return out;
                    }
                    out.push(c);
                }
                b'\\' => {
                    let Some(&e) = d.get(i) else { break };
                    i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match d.get(i) {
                                    Some(&o @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(o - b'0');
                                        i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        b'\r' => {
                            if d.get(i) == Some(&b'\n') {
                                i += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                b'\r' => {
                    if d.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(c),
            }
        }
        self.pos = d.len();
        self.error(start, ParseErrorKind::UnterminatedString);
        out
    }

    fn scan_hex_string(&mut self, start: usize) -> Vec<u8> {
        let d = self.data;
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        let mut bad = false;
        let mut i = self.pos;
        while i < d.len() {
            let c = d[i];
            i += 1;
            if c == b'>' {
                self.pos = i;
                if let Some(h) = hi {
                    out.push(h << 4);
                }
                if bad {
                    self.error(start, ParseErrorKind::BadHexDigit);
                }
                return out;
            }
            match hex_val(c) {
                Some(v) => match hi.take() {
                    Some(h) => out.push(h << 4 | v),
                    None => hi = Some(v),
                },
                None => {
                    if !is_whitespace(c) {
                        bad = true;
                    }
                }
            }
        }
        self.pos = d.len();
        if let Some(h) = hi {
            out.push(h << 4);
        }
        self.error(start, ParseErrorKind::UnterminatedString);
        out
    }
}

enum Num {
    Int(i64),
    Real(f64),
}

fn number_from_f64_or_int(n: Num) -> Object {
    match n {
        Num::Int(i) => Object::Int(i),
        Num::Real(r) => number_from_f64(r),
    }
}

const POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// Lenient PDF number: any run of `+`/`-` signs (a `-` anywhere in the run
/// negates, so `--5` is -5), digits with at most one `.`, at least one digit.
fn classify_number(s: &[u8]) -> Option<Num> {
    let first = *s.first()?;
    if !(first.is_ascii_digit() || matches!(first, b'+' | b'-' | b'.')) {
        return None;
    }
    let mut i = 0;
    let mut neg = false;
    while i < s.len() && matches!(s[i], b'+' | b'-') {
        neg |= s[i] == b'-';
        i += 1;
    }
    let body = &s[i..];
    let mut mant: u64 = 0;
    let mut digits = 0usize;
    let mut frac_digits = 0usize;
    let mut seen_dot = false;
    let mut overflow = false;
    for &c in body {
        match c {
            b'0'..=b'9' => {
                digits += 1;
                if seen_dot {
                    frac_digits += 1;
                }
                match mant
                    .checked_mul(10)
                    .and_then(|m| m.checked_add(u64::from(c - b'0')))
                {
                    Some(m) => mant = m,
                    None => overflow = true,
                }
            }
            b'.' if !seen_dot => seen_dot = true,
            _ => return None,
        }
    }
    if digits == 0 {
        return None;
    }
    if !seen_dot {
        if !overflow && mant as f64 <= MAX_INT {
            let v = mant as i64;
            return Some(Num::Int(if neg { -v } else { v }));
        }
    } else if !overflow && digits <= 15 && frac_digits <= 22 {
        let v = mant as f64 / POW10[frac_digits];
        return Some(Num::Real(if neg { -v } else { v }));
    }
    // Slow path: let std do correct rounding on the sanitized text.
    let text = std::str::from_utf8(body).ok()?;
    let v: f64 = text.parse().ok()?;
    Some(Num::Real(if neg { -v } else { v }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(s: &[u8]) -> Vec<TokenKind> {
        let mut l = Lexer::new(s);
        let mut v = Vec::new();
        while let Some(t) = l.next_token() {
            v.push(t.kind);
        }
        v
    }

    fn one(s: &[u8]) -> TokenKind {
        let v = lex(s);
        assert_eq!(v.len(), 1, "{:?}", v);
        v.into_iter().next().unwrap()
    }

    #[test]
    fn numbers_odd_forms() {
        assert_eq!(one(b"-.5"), TokenKind::Real(-0.5));
        assert_eq!(one(b"+.5"), TokenKind::Real(0.5));
        assert_eq!(one(b"4."), TokenKind::Int(4));
        assert_eq!(one(b"4.0"), TokenKind::Int(4));
        assert_eq!(one(b"--5"), TokenKind::Int(-5));
        assert_eq!(one(b"-0"), TokenKind::Int(0));
        assert_eq!(one(b"007"), TokenKind::Int(7));
        assert_eq!(one(b"123.456"), TokenKind::Real(123.456));
        assert_eq!(one(b"0.1"), TokenKind::Real(0.1));
        assert_eq!(
            one(b"99999999999999999999"),
            TokenKind::Real(99999999999999999999.0)
        );
        assert_eq!(
            one(b"0.12345678901234567890"),
            TokenKind::Real(0.123_456_789_012_345_68)
        );
    }

    #[test]
    fn non_numbers_are_keywords() {
        for k in [
            &b"-"[..],
            b".",
            b"1.2.3",
            b"12abc",
            b"5-3",
            b"1e5",
            b"T*",
            b"'",
            b"\"",
            b"b*",
        ] {
            assert_eq!(
                one(k),
                TokenKind::Keyword,
                "{:?}",
                String::from_utf8_lossy(k)
            );
        }
    }

    #[test]
    fn literal_strings() {
        assert_eq!(one(b"(a(b)c)"), TokenKind::LiteralString(b"a(b)c".to_vec()));
        assert_eq!(
            one(br"(a\)b\\c)"),
            TokenKind::LiteralString(b"a)b\\c".to_vec())
        );
        assert_eq!(
            one(br"(\n\r\t\b\f)"),
            TokenKind::LiteralString(vec![10, 13, 9, 8, 12])
        );
        assert_eq!(
            one(br"(\101\60\0053)"),
            TokenKind::LiteralString(vec![b'A', b'0', 5, b'3'])
        );
        assert_eq!(one(br"(\777)"), TokenKind::LiteralString(vec![0xFF]));
        assert_eq!(one(b"(a\\\nb)"), TokenKind::LiteralString(b"ab".to_vec()));
        assert_eq!(one(b"(a\\\r\nb)"), TokenKind::LiteralString(b"ab".to_vec()));
        assert_eq!(
            one(b"(a\r\nb\rc)"),
            TokenKind::LiteralString(b"a\nb\nc".to_vec())
        );
        assert_eq!(one(br"(\q)"), TokenKind::LiteralString(b"q".to_vec()));
        assert_eq!(one(b"()"), TokenKind::LiteralString(vec![]));
    }

    #[test]
    fn unterminated_string_reports_error() {
        let mut l = Lexer::new(b"(abc");
        let t = l.next_token().unwrap();
        assert_eq!(t.kind, TokenKind::LiteralString(b"abc".to_vec()));
        assert_eq!(l.errors()[0].kind, ParseErrorKind::UnterminatedString);
        assert_eq!(l.errors()[0].offset, 0);
    }

    #[test]
    fn hex_strings() {
        assert_eq!(
            one(b"<48 65 6C6c6F>"),
            TokenKind::HexString(b"Hello".to_vec())
        );
        assert_eq!(one(b"<4>"), TokenKind::HexString(vec![0x40]));
        assert_eq!(one(b"<>"), TokenKind::HexString(vec![]));
        let mut l = Lexer::new(b"<4g1>");
        assert_eq!(
            l.next_token().unwrap().kind,
            TokenKind::HexString(vec![0x41])
        );
        assert_eq!(l.errors().len(), 1);
    }

    #[test]
    fn names() {
        assert_eq!(one(b"/Name"), TokenKind::Name(b"Name".to_vec()));
        assert_eq!(one(b"/A#20B"), TokenKind::Name(b"A B".to_vec()));
        assert_eq!(one(b"/A#2"), TokenKind::Name(b"A#2".to_vec()));
        assert_eq!(one(b"/A#"), TokenKind::Name(b"A#".to_vec()));
        assert_eq!(one(b"/#41#zz"), TokenKind::Name(b"A#zz".to_vec()));
        assert_eq!(one(b"/"), TokenKind::Name(vec![]));
        assert_eq!(
            lex(b"/A/B"),
            vec![
                TokenKind::Name(b"A".to_vec()),
                TokenKind::Name(b"B".to_vec())
            ]
        );
    }

    #[test]
    fn containers_comments_and_stray() {
        use TokenKind::*;
        assert_eq!(
            lex(b"[1 2]<</K 1>>"),
            vec![
                ArrayStart,
                Int(1),
                Int(2),
                ArrayEnd,
                DictStart,
                Name(b"K".to_vec()),
                Int(1),
                DictEnd
            ]
        );
        assert_eq!(lex(b"% hi\n q"), vec![Comment, Keyword]);
        assert_eq!(
            lex(b") > { }"),
            vec![Stray(b')'), Stray(b'>'), Stray(b'{'), Stray(b'}')]
        );
    }

    #[test]
    fn non_printable_bytes_split_keywords() {
        use TokenKind::*;
        assert_eq!(
            lex(b"ab\xffcd \x01"),
            vec![Keyword, Stray(0xff), Keyword, Stray(1)]
        );
        // Names keep raw high bytes.
        assert_eq!(lex(b"/F\xc3\xb6"), vec![Name(vec![b'F', 0xc3, 0xb6])]);
    }

    #[test]
    fn spans_are_exact() {
        let mut l = Lexer::new(b"  12 (ab) Tj");
        let spans: Vec<_> = std::iter::from_fn(|| l.next_token())
            .map(|t| t.span)
            .collect();
        assert_eq!(spans, vec![2..4, 5..9, 10..12]);
    }

    #[test]
    fn never_panics_on_all_bytes() {
        for b in 0..=255u8 {
            let _ = lex(&[b]);
            let _ = lex(&[b'(', b]);
            let _ = lex(&[b'<', b]);
            let _ = lex(&[b'/', b, b'#']);
            let _ = lex(&[b'\\', b, b'-', b'.']);
        }
    }
}
