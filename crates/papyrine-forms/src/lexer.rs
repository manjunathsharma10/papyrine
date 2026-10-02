//! A small JavaScript tokenizer: strings with escapes, numbers, identifiers,
//! punctuation and comments. It never panics; malformed input is an `Err`.

/// Upper bound on accepted script size (bytes). Larger inputs are rejected.
pub const MAX_SCRIPT_BYTES: usize = 1 << 20;
/// Upper bound on tokens per script.
pub const MAX_TOKENS: usize = 200_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    /// A single punctuation character. Multi-character operators are emitted
    /// as consecutive tokens, which is all the recognizer needs.
    Punct(char),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    /// A line terminator (or comment containing one) precedes this token.
    pub nl_before: bool,
    /// Byte offset of the token start.
    pub pos: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    TooLarge,
    TooManyTokens,
    UnterminatedString(usize),
    UnterminatedComment(usize),
    BadEscape(usize),
    BadNumber(usize),
    /// Template literals, `#`, `@`, stray backslashes and similar.
    UnsupportedChar(usize, char),
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexError::TooLarge => write!(f, "script too large"),
            LexError::TooManyTokens => write!(f, "too many tokens"),
            LexError::UnterminatedString(p) => write!(f, "unterminated string at {p}"),
            LexError::UnterminatedComment(p) => write!(f, "unterminated comment at {p}"),
            LexError::BadEscape(p) => write!(f, "bad escape at {p}"),
            LexError::BadNumber(p) => write!(f, "bad number at {p}"),
            LexError::UnsupportedChar(p, c) => write!(f, "unsupported character {c:?} at {p}"),
        }
    }
}

impl std::error::Error for LexError {}

fn is_ident_start(c: char) -> bool {
    c == '$' || c == '_' || c.is_alphabetic()
}

fn is_ident_part(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '\u{200c}' || c == '\u{200d}'
}

fn is_line_term(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}' || c == '\0'
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, LexError> {
    if src.len() > MAX_SCRIPT_BYTES {
        return Err(LexError::TooLarge);
    }
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let mut nl = false;
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let pos_of = |i: usize| chars.get(i).map(|&(p, _)| p).unwrap_or(src.len());

    while i < chars.len() {
        let (pos, c) = chars[i];
        if out.len() > MAX_TOKENS {
            return Err(LexError::TooManyTokens);
        }
        if is_line_term(c) {
            nl = true;
            i += 1;
        } else if is_space(c) {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            while i < chars.len() && !is_line_term(chars[i].1) {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let start = pos;
            i += 2;
            loop {
                match (at(i), at(i + 1)) {
                    (Some('*'), Some('/')) => {
                        i += 2;
                        break;
                    }
                    (Some(ch), _) => {
                        if is_line_term(ch) {
                            nl = true;
                        }
                        i += 1;
                    }
                    (None, _) => return Err(LexError::UnterminatedComment(start)),
                }
            }
        } else if c == '"' || c == '\'' {
            let quote = c;
            let start = pos;
            i += 1;
            let mut s = String::new();
            loop {
                let Some(ch) = at(i) else {
                    return Err(LexError::UnterminatedString(start));
                };
                i += 1;
                if ch == quote {
                    break;
                }
                if is_line_term(ch) {
                    return Err(LexError::UnterminatedString(start));
                }
                if ch != '\\' {
                    s.push(ch);
                    continue;
                }
                let epos = pos_of(i - 1);
                let Some(e) = at(i) else {
                    return Err(LexError::UnterminatedString(start));
                };
                i += 1;
                match e {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    'v' => s.push('\u{b}'),
                    '0' if !at(i).is_some_and(|d| d.is_ascii_digit()) => s.push('\0'),
                    '0'..='7' => {
                        // Legacy octal escape (up to three digits, value <= 255).
                        let mut v = e.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match at(i).and_then(|d| d.to_digit(8)) {
                                Some(d) if v * 8 + d <= 255 => {
                                    v = v * 8 + d;
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        s.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                    }
                    'x' => {
                        let h = hex_n(&chars, i, 2).ok_or(LexError::BadEscape(epos))?;
                        i += 2;
                        s.push(char::from_u32(h).unwrap_or('\u{fffd}'));
                    }
                    'u' => {
                        let h = hex_n(&chars, i, 4).ok_or(LexError::BadEscape(epos))?;
                        i += 4;
                        if (0xD800..0xDC00).contains(&h)
                            && at(i) == Some('\\')
                            && at(i + 1) == Some('u')
                            && let Some(lo) = hex_n(&chars, i + 2, 4)
                            && (0xDC00..0xE000).contains(&lo)
                        {
                            i += 6;
                            let cp = 0x10000 + ((h - 0xD800) << 10) + (lo - 0xDC00);
                            s.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        } else {
                            s.push(char::from_u32(h).unwrap_or('\u{fffd}'));
                        }
                    }
                    '\r' => {
                        if at(i) == Some('\n') {
                            i += 1;
                        }
                    }
                    '\n' | '\u{2028}' | '\u{2029}' => {}
                    other => s.push(other),
                }
            }
            out.push(Token {
                tok: Tok::Str(s),
                nl_before: std::mem::take(&mut nl),
                pos: start,
            });
        } else if c.is_ascii_digit() || (c == '.' && at(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            let start = pos;
            let begin = i;
            let value = if c == '0' && matches!(at(i + 1), Some('x' | 'X')) {
                i += 2;
                let ds = i;
                while at(i).is_some_and(|d| d.is_ascii_hexdigit()) {
                    i += 1;
                }
                if ds == i {
                    return Err(LexError::BadNumber(start));
                }
                let text: String = chars[ds..i].iter().map(|&(_, c)| c).collect();
                u128::from_str_radix(&text, 16)
                    .map(|v| v as f64)
                    .unwrap_or(f64::INFINITY)
            } else {
                while at(i).is_some_and(|d| d.is_ascii_digit()) {
                    i += 1;
                }
                if at(i) == Some('.') {
                    i += 1;
                    while at(i).is_some_and(|d| d.is_ascii_digit()) {
                        i += 1;
                    }
                }
                if matches!(at(i), Some('e' | 'E')) {
                    let mut j = i + 1;
                    if matches!(at(j), Some('+' | '-')) {
                        j += 1;
                    }
                    if at(j).is_some_and(|d| d.is_ascii_digit()) {
                        while at(j).is_some_and(|d| d.is_ascii_digit()) {
                            j += 1;
                        }
                        i = j;
                    } else {
                        return Err(LexError::BadNumber(start));
                    }
                }
                let text: String = chars[begin..i].iter().map(|&(_, c)| c).collect();
                text.parse::<f64>()
                    .map_err(|_| LexError::BadNumber(start))?
            };
            if at(i).is_some_and(is_ident_start) {
                return Err(LexError::BadNumber(start));
            }
            out.push(Token {
                tok: Tok::Num(value),
                nl_before: std::mem::take(&mut nl),
                pos: start,
            });
        } else if is_ident_start(c) {
            let start = pos;
            let begin = i;
            while at(i).is_some_and(is_ident_part) {
                i += 1;
            }
            let text: String = chars[begin..i].iter().map(|&(_, c)| c).collect();
            out.push(Token {
                tok: Tok::Ident(text),
                nl_before: std::mem::take(&mut nl),
                pos: start,
            });
        } else if c.is_ascii_punctuation() && !matches!(c, '`' | '#' | '@' | '\\') {
            out.push(Token {
                tok: Tok::Punct(c),
                nl_before: std::mem::take(&mut nl),
                pos,
            });
            i += 1;
        } else {
            return Err(LexError::UnsupportedChar(pos, c));
        }
    }
    Ok(out)
}

fn hex_n(chars: &[(usize, char)], at: usize, n: usize) -> Option<u32> {
    let mut v = 0u32;
    for k in 0..n {
        v = v * 16 + chars.get(at + k)?.1.to_digit(16)?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn basic_call() {
        assert_eq!(
            toks("AFNumber_Format(2, 0, \"$\");"),
            vec![
                Tok::Ident("AFNumber_Format".into()),
                Tok::Punct('('),
                Tok::Num(2.0),
                Tok::Punct(','),
                Tok::Num(0.0),
                Tok::Punct(','),
                Tok::Str("$".into()),
                Tok::Punct(')'),
                Tok::Punct(';'),
            ]
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(
            toks(r#"'a\n\x41B\'\101'"#),
            vec![Tok::Str("a\nAB'A".into())]
        );
        assert_eq!(toks(r#""😀""#), vec![Tok::Str("😀".into())]);
        assert_eq!(toks("'a\\\nb'"), vec![Tok::Str("ab".into())]);
    }

    #[test]
    fn numbers() {
        assert_eq!(toks("1 .5 2.5e2 0x1F 3."), {
            vec![
                Tok::Num(1.0),
                Tok::Num(0.5),
                Tok::Num(250.0),
                Tok::Num(31.0),
                Tok::Num(3.0),
            ]
        });
        assert!(tokenize("3in").is_err());
        assert!(tokenize("1e").is_err());
    }

    #[test]
    fn comments_and_newlines() {
        let t = tokenize("a // x\n/* y\n z */ b /* c */ d").unwrap();
        assert_eq!(t.len(), 3);
        assert!(!t[0].nl_before);
        assert!(t[1].nl_before);
        assert!(!t[2].nl_before);
    }

    #[test]
    fn errors() {
        assert!(matches!(
            tokenize("'abc"),
            Err(LexError::UnterminatedString(_))
        ));
        assert!(matches!(
            tokenize("/* abc"),
            Err(LexError::UnterminatedComment(_))
        ));
        assert!(matches!(tokenize("'\\xZZ'"), Err(LexError::BadEscape(_))));
        assert!(matches!(
            tokenize("`x`"),
            Err(LexError::UnsupportedChar(..))
        ));
        assert!(matches!(
            tokenize("'a\nb'"),
            Err(LexError::UnterminatedString(_))
        ));
    }
}
