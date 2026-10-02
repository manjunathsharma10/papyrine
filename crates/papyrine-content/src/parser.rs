//! Parser from tokens to [`Op`]s. Tolerant: every problem becomes a
//! [`ParseError`] and parsing continues.

use crate::lexer::{Lexer, Token, TokenKind, is_delimiter, is_whitespace};
use crate::object::{Object, Op};
use std::fmt;

/// Maximum nesting of arrays and dictionaries; deeper brackets are errors.
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    UnterminatedString,
    BadHexDigit,
    StrayDelimiter(u8),
    /// Control or non-ASCII byte where a token should start.
    InvalidByte(u8),
    UnterminatedArray,
    UnterminatedDict,
    /// An operator appeared inside an array or dictionary; the container was closed there.
    OperatorInContainer,
    DictKeyNotName,
    DictKeyWithoutValue,
    TooDeep,
    /// Operands at end of input with no operator.
    TrailingOperands(usize),
    InlineImageNoData,
    InlineImageNoEnd,
    /// A word inside an inline-image dictionary other than `ID`.
    UnexpectedInInlineImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub kind: ParseErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "offset {}: {:?}", self.offset, self.kind)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Default)]
pub struct Parsed {
    pub ops: Vec<Op>,
    pub errors: Vec<ParseError>,
}

/// Parse a whole content stream.
pub fn parse(data: &[u8]) -> Parsed {
    let mut p = Parser::new(data);
    let mut ops = Vec::new();
    while let Some(op) = p.next_op() {
        ops.push(op);
    }
    Parsed {
        ops,
        errors: p.finish(),
    }
}

pub struct Parser<'a> {
    lex: Lexer<'a>,
    operands: Vec<Object>,
    operand_start: Option<usize>,
}

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            lex: Lexer::new(data),
            operands: Vec::new(),
            operand_start: None,
        }
    }

    /// Errors collected so far, plus a final check for dangling operands.
    pub fn finish(mut self) -> Vec<ParseError> {
        if !self.operands.is_empty() {
            let offset = self.operand_start.unwrap_or(self.lex.data().len());
            let n = self.operands.len();
            self.lex.error(offset, ParseErrorKind::TrailingOperands(n));
        }
        let mut errs = self.lex.take_errors();
        errs.sort_by_key(|e| e.offset);
        errs
    }

    /// Next complete operation, or `None` at end of input.
    pub fn next_op(&mut self) -> Option<Op> {
        loop {
            let tok = self.lex.next_token()?;
            let start = tok.span.start;
            match tok.kind {
                TokenKind::Comment | TokenKind::Stray(_) => {}
                TokenKind::Keyword => {
                    let word = &self.lex.data()[tok.span.clone()];
                    match word {
                        b"true" => self.push_operand(start, Object::Bool(true)),
                        b"false" => self.push_operand(start, Object::Bool(false)),
                        b"null" => self.push_operand(start, Object::Null),
                        b"BI" => {
                            if let Some(op) = self.inline_image(start) {
                                return Some(op);
                            }
                        }
                        _ => {
                            let op = Op {
                                operator: word.to_vec(),
                                operands: std::mem::take(&mut self.operands),
                                inline_data: None,
                                span: self.operand_start.take().unwrap_or(start)..tok.span.end,
                            };
                            return Some(op);
                        }
                    }
                }
                TokenKind::ArrayEnd => {
                    self.lex.error(start, ParseErrorKind::StrayDelimiter(b']'));
                }
                TokenKind::DictEnd => {
                    self.lex.error(start, ParseErrorKind::StrayDelimiter(b'>'));
                }
                _ => {
                    if let Some(obj) = self.object_from(tok, 0) {
                        self.push_operand(start, obj);
                    }
                }
            }
        }
    }

    fn push_operand(&mut self, start: usize, obj: Object) {
        if self.operands.is_empty() {
            self.operand_start = Some(start);
        }
        self.operands.push(obj);
    }

    /// Build an object from a value-starting token. `None` if the token is not
    /// a value (already reported).
    fn object_from(&mut self, tok: Token, depth: usize) -> Option<Object> {
        Some(match tok.kind {
            TokenKind::Int(i) => Object::Int(i),
            TokenKind::Real(r) => Object::Real(r),
            TokenKind::LiteralString(s) | TokenKind::HexString(s) => Object::Str(s),
            TokenKind::Name(n) => Object::Name(n),
            TokenKind::ArrayStart => {
                if depth >= MAX_DEPTH {
                    self.lex.error(tok.span.start, ParseErrorKind::TooDeep);
                    return None;
                }
                Object::Array(self.container_items(tok.span.start, depth + 1, false))
            }
            TokenKind::DictStart => {
                if depth >= MAX_DEPTH {
                    self.lex.error(tok.span.start, ParseErrorKind::TooDeep);
                    return None;
                }
                let items = self.container_items(tok.span.start, depth + 1, true);
                self.items_to_dict(items)
            }
            _ => return None,
        })
    }

    /// Items up to the matching `]` (array) or `>>` (dict).
    fn container_items(&mut self, open: usize, depth: usize, is_dict: bool) -> Vec<Object> {
        let mut items = Vec::new();
        loop {
            let Some(tok) = self.lex.next_token() else {
                let kind = if is_dict {
                    ParseErrorKind::UnterminatedDict
                } else {
                    ParseErrorKind::UnterminatedArray
                };
                self.lex.error(open, kind);
                return items;
            };
            let start = tok.span.start;
            match tok.kind {
                TokenKind::ArrayEnd if !is_dict => return items,
                TokenKind::DictEnd if is_dict => return items,
                TokenKind::ArrayEnd => self.lex.error(start, ParseErrorKind::StrayDelimiter(b']')),
                TokenKind::DictEnd => self.lex.error(start, ParseErrorKind::StrayDelimiter(b'>')),
                TokenKind::Comment | TokenKind::Stray(_) => {}
                TokenKind::Keyword => {
                    match &self.lex.data()[tok.span.clone()] {
                        b"true" => items.push(Object::Bool(true)),
                        b"false" => items.push(Object::Bool(false)),
                        b"null" => items.push(Object::Null),
                        _ => {
                            // Close the container and let the caller see the operator.
                            self.lex.error(start, ParseErrorKind::OperatorInContainer);
                            let kind = if is_dict {
                                ParseErrorKind::UnterminatedDict
                            } else {
                                ParseErrorKind::UnterminatedArray
                            };
                            self.lex.error(open, kind);
                            self.lex.set_pos(start);
                            return items;
                        }
                    }
                }
                _ => {
                    if let Some(o) = self.object_from(tok, depth) {
                        items.push(o);
                    }
                }
            }
        }
    }

    fn items_to_dict(&mut self, items: Vec<Object>) -> Object {
        let mut pairs = Vec::with_capacity(items.len() / 2);
        let mut it = items.into_iter();
        while let Some(k) = it.next() {
            let Object::Name(key) = k else {
                self.lex
                    .error(self.lex.pos(), ParseErrorKind::DictKeyNotName);
                continue;
            };
            match it.next() {
                Some(v) => pairs.push((key, v)),
                None => self
                    .lex
                    .error(self.lex.pos(), ParseErrorKind::DictKeyWithoutValue),
            }
        }
        Object::Dict(pairs)
    }

    fn inline_image(&mut self, bi_start: usize) -> Option<Op> {
        // BI takes no operands; anything pending before it is garbage.
        let mut items = Vec::new();
        loop {
            let Some(tok) = self.lex.next_token() else {
                self.lex.error(bi_start, ParseErrorKind::InlineImageNoData);
                return None;
            };
            match tok.kind {
                TokenKind::Keyword => match &self.lex.data()[tok.span.clone()] {
                    b"ID" => break,
                    b"true" => items.push(Object::Bool(true)),
                    b"false" => items.push(Object::Bool(false)),
                    b"null" => items.push(Object::Null),
                    _ => self
                        .lex
                        .error(tok.span.start, ParseErrorKind::UnexpectedInInlineImage),
                },
                TokenKind::Comment | TokenKind::Stray(_) => {}
                TokenKind::ArrayEnd | TokenKind::DictEnd => self
                    .lex
                    .error(tok.span.start, ParseErrorKind::StrayDelimiter(b']')),
                _ => {
                    if let Some(o) = self.object_from(tok, 0) {
                        items.push(o);
                    }
                }
            }
        }
        let dict = self.items_to_dict(items);
        let data = self.lex.data();
        let pos = self.lex.pos();
        let Some((data_range, end)) = find_inline_data(data, pos, &dict) else {
            // Without a terminator the image cannot be re-serialized faithfully.
            self.lex.error(bi_start, ParseErrorKind::InlineImageNoEnd);
            self.lex.set_pos(data.len());
            return None;
        };
        self.lex.set_pos(end);
        self.operands.clear();
        self.operand_start = None;
        Some(Op {
            operator: b"BI".to_vec(),
            operands: vec![dict],
            inline_data: Some(data[data_range].to_vec()),
            span: bi_start..end,
        })
    }
}

fn dict_get<'d>(dict: &'d Object, keys: &[&str]) -> Option<&'d Object> {
    let Object::Dict(pairs) = dict else {
        return None;
    };
    pairs
        .iter()
        .find(|(k, _)| keys.iter().any(|w| w.as_bytes() == k.as_slice()))
        .map(|(_, v)| v)
}

/// Byte length of the image data when it can be derived from the dictionary.
fn expected_data_len(dict: &Object) -> Option<usize> {
    if let Some(Object::Int(l)) = dict_get(dict, &["L", "Length"]) {
        return usize::try_from(*l).ok();
    }
    if dict_get(dict, &["F", "Filter"]).is_some() {
        return None;
    }
    let w = dict_get(dict, &["W", "Width"])?.as_i64()?;
    let h = dict_get(dict, &["H", "Height"])?.as_i64()?;
    let is_mask = matches!(
        dict_get(dict, &["IM", "ImageMask"]),
        Some(Object::Bool(true))
    );
    let (bpc, ncomp) = if is_mask {
        (1, 1)
    } else {
        let bpc = dict_get(dict, &["BPC", "BitsPerComponent"])?.as_i64()?;
        let ncomp = match dict_get(dict, &["CS", "ColorSpace"])? {
            Object::Name(n) => match n.as_slice() {
                b"G" | b"DeviceGray" | b"CalGray" => 1,
                b"RGB" | b"DeviceRGB" | b"CalRGB" => 3,
                b"CMYK" | b"DeviceCMYK" => 4,
                _ => return None,
            },
            Object::Array(a) => match a.first().and_then(Object::as_name) {
                Some(b"I" | b"Indexed") => 1,
                _ => return None,
            },
            _ => return None,
        };
        (bpc, ncomp)
    };
    if w <= 0 || h <= 0 || !(1..=16).contains(&bpc) {
        return None;
    }
    let row_bits = (w as u128) * (bpc as u128) * ncomp;
    let row_bytes = row_bits.div_ceil(8);
    usize::try_from(row_bytes * h as u128).ok()
}

fn ends_token(data: &[u8], i: usize) -> bool {
    data.get(i)
        .is_none_or(|&b| is_whitespace(b) || is_delimiter(b))
}

/// Is there an `EI` keyword at `i`, after at most one whitespace byte (or CRLF)?
/// A longer gap is rejected because binary data is often whitespace-like.
fn ei_after_ws(data: &[u8], mut i: usize) -> Option<usize> {
    if data.get(i) == Some(&b'\r') && data.get(i + 1) == Some(&b'\n') {
        i += 2;
    } else if data.get(i).is_some_and(|&b| is_whitespace(b)) {
        i += 1;
    }
    (data.get(i..i + 2) == Some(b"EI") && ends_token(data, i + 2)).then_some(i + 2)
}

/// Locate inline-image data after `ID`. `pos` is just past the `ID` keyword.
/// Returns the data range and the offset just past `EI`.
fn find_inline_data(
    data: &[u8],
    pos: usize,
    dict: &Object,
) -> Option<(std::ops::Range<usize>, usize)> {
    // Exactly one whitespace byte follows ID (CRLF counts as one separator when
    // the declared length confirms it). Writers that omit it are tolerated.
    let seps: &[usize] = match (data.get(pos), data.get(pos + 1)) {
        (Some(b'\r'), Some(b'\n')) => &[2, 1],
        (Some(&b), _) if is_whitespace(b) => &[1],
        _ => &[0],
    };
    if let Some(n) = expected_data_len(dict) {
        for &sep in seps {
            let start = pos + sep;
            if let Some(end) = start.checked_add(n)
                && end <= data.len()
                && let Some(after) = ei_after_ws(data, end)
            {
                return Some((start..end, after));
            }
        }
    }
    let start = pos + seps[seps.len() - 1];
    let mut i = start;
    while i + 1 < data.len() {
        if data[i] == b'E'
            && data[i + 1] == b'I'
            && (i == start || is_whitespace(data[i - 1]))
            && ends_token(data, i + 2)
            && looks_like_text(&data[(i + 2).min(data.len())..])
        {
            let end = if i > start { i - 1 } else { start };
            return Some((start..end, i + 2));
        }
        i += 1;
    }
    None
}

/// Heuristic from common readers: after a real `EI`, the next bytes are
/// operators/operands, not binary image data. Six bytes is deliberate: it
/// stops before the data of an immediately following inline image
/// (`\nBI\nID ` is seven bytes), which our own serializer can produce.
fn looks_like_text(rest: &[u8]) -> bool {
    rest.iter()
        .take(6)
        .all(|&b| b == 9 || b == 10 || b == 13 || (32..=126).contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(s: &[u8]) -> Vec<Op> {
        parse(s).ops
    }

    #[test]
    fn basic_ops_and_spans() {
        let src = b"q 1 0 0 1 10 20 cm\n/F1 12 Tf (Hi) Tj Q";
        let p = parse(src);
        assert!(p.errors.is_empty());
        let names: Vec<_> = p
            .ops
            .iter()
            .map(|o| o.operator_str().into_owned())
            .collect();
        assert_eq!(names, ["q", "cm", "Tf", "Tj", "Q"]);
        assert_eq!(p.ops[1].operands.len(), 6);
        assert_eq!(&src[p.ops[1].span.clone()], b"1 0 0 1 10 20 cm");
        assert_eq!(&src[p.ops[0].span.clone()], b"q");
        assert_eq!(&src[p.ops[3].span.clone()], b"(Hi) Tj");
        assert_eq!(p.ops[2].operands, vec![Object::name("F1"), Object::Int(12)]);
    }

    #[test]
    fn arrays_dicts_and_keywords() {
        let o = ops(b"[(A) -120 (B) true null] TJ /Span <</MCID 3 /Lang (en) /X [1 2]>> BDC");
        assert_eq!(
            o[0].operands,
            vec![Object::Array(vec![
                Object::string("A"),
                Object::Int(-120),
                Object::string("B"),
                Object::Bool(true),
                Object::Null
            ])]
        );
        assert_eq!(
            o[1].operands[1],
            Object::Dict(vec![
                (b"MCID".to_vec(), Object::Int(3)),
                (b"Lang".to_vec(), Object::string("en")),
                (
                    b"X".to_vec(),
                    Object::Array(vec![Object::Int(1), Object::Int(2)])
                ),
            ])
        );
    }

    #[test]
    fn operator_inside_array_closes_it() {
        let p = parse(b"[1 2 Tj 3 4 re");
        assert_eq!(p.ops.len(), 2);
        assert_eq!(p.ops[0].operator, b"Tj");
        assert_eq!(p.ops[1].operator, b"re");
        assert!(
            p.errors
                .iter()
                .any(|e| e.kind == ParseErrorKind::OperatorInContainer)
        );
    }

    #[test]
    fn garbage_is_tolerated() {
        let p = parse(b") ] >> 1 2 { m (unterminated");
        assert!(!p.errors.is_empty());
        let p = parse(b"1 2 3");
        assert!(p.ops.is_empty());
        assert_eq!(p.errors[0].kind, ParseErrorKind::TrailingOperands(3));
        let p = parse(b"<< /A 1 /B >> BDC <</1 2>> x");
        assert_eq!(p.ops.len(), 2);
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut s = vec![b'['; 5000];
        s.extend(vec![b']'; 5000]);
        s.extend(b" TJ");
        let p = parse(&s);
        assert!(p.errors.iter().any(|e| e.kind == ParseErrorKind::TooDeep));
    }

    #[test]
    fn inline_image_known_length() {
        let mut s = b"q BI /W 2 /H 2 /BPC 8 /CS /G ID ".to_vec();
        // Data contains a fake "EI" token on purpose.
        s.extend(b" EI ");
        s.extend(b" EI Q");
        let p = parse(&s);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.ops.len(), 3);
        assert_eq!(p.ops[1].inline_data.as_deref(), Some(&b" EI "[..]));
        assert_eq!(p.ops[2].operator, b"Q");
    }

    #[test]
    fn inline_image_heuristic_with_filter() {
        let mut s = b"BI /W 4 /H 4 /BPC 8 /CS /RGB /F /Fl ID ".to_vec();
        s.extend([0x78, 0x9c, 0xff, 0x00, 0x80, b'E', b'I', 0xfe, 0x01]);
        s.extend(b"\nEI\nQ");
        let p = parse(&s);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.ops.len(), 2);
        assert_eq!(
            p.ops[0].inline_data.as_deref(),
            Some(&[0x78, 0x9c, 0xff, 0x00, 0x80, b'E', b'I', 0xfe, 0x01][..])
        );
        assert_eq!(&s[p.ops[0].span.clone()], &s[..s.len() - 2]);
    }

    #[test]
    fn inline_image_unterminated_and_missing_id() {
        let p = parse(b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x00");
        assert!(p.ops.is_empty());
        assert!(
            p.errors
                .iter()
                .any(|e| e.kind == ParseErrorKind::InlineImageNoEnd)
        );
        let p = parse(b"BI /W 1");
        assert!(p.ops.is_empty());
    }

    #[test]
    fn inline_image_explicit_length() {
        let p = parse(b"BI /W 1 /H 1 /L 3 /F /AHx ID abc EI Q");
        assert_eq!(p.ops[0].inline_data.as_deref(), Some(&b"abc"[..]));
        assert_eq!(p.ops[1].operator, b"Q");
    }

    #[test]
    fn inline_image_crlf_after_id() {
        let p = parse(b"BI /W 2 /H 1 /BPC 8 /CS /G ID\r\nab\nEI Q");
        assert_eq!(p.ops[0].inline_data.as_deref(), Some(&b"ab"[..]));
    }
}
