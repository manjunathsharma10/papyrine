//! Adobe's viewer-version / XFA-check document scripts.
//!
//! Hundreds of public forms (all IRS XFA hybrids, for example) carry a
//! standard set of document-level scripts whose only job is to nag users of
//! an old Acrobat to upgrade. They are recognised by a fingerprint of the
//! *normalised token stream*, never by substring: string literals and numbers
//! are replaced by placeholders (messages and version thresholds differ per
//! form and locale), identifiers and punctuation are kept, comments and
//! whitespace vanish in the tokenizer. A matching script is treated as a
//! no-op and is never executed.

use crate::lexer::{Tok, tokenize};

/// Length in tokens of an `ADBE . name = "str" ;?` statement at the start of `t`.
fn adbe_string_assign(t: &[crate::lexer::Token]) -> Option<usize> {
    let ok = matches!(t.first().map(|x| &x.tok), Some(Tok::Ident(a)) if a == "ADBE")
        && matches!(t.get(1).map(|x| &x.tok), Some(Tok::Punct('.')))
        && matches!(t.get(2).map(|x| &x.tok), Some(Tok::Ident(_)))
        && matches!(t.get(3).map(|x| &x.tok), Some(Tok::Punct('=')))
        && matches!(t.get(4).map(|x| &x.tok), Some(Tok::Str(_)));
    if !ok {
        return None;
    }
    Some(
        if matches!(t.get(5).map(|x| &x.tok), Some(Tok::Punct(';'))) {
            6
        } else {
            5
        },
    )
}

/// FNV-1a over the normalised token stream; `None` if the text does not lex.
pub fn fingerprint(src: &str) -> Option<u64> {
    let toks = tokenize(src).ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let mut i = 0;
    let mut in_string_run = false;
    while i < toks.len() {
        // `ADBE.<name> = "<string>";` statements collapse to one marker per
        // run, so the string table may gain or lose entries between versions.
        if let Some(n) = adbe_string_assign(&toks[i..]) {
            if !in_string_run {
                feed(b"ADBE-STRINGS");
                in_string_run = true;
            }
            i += n;
            continue;
        }
        in_string_run = false;
        match &toks[i].tok {
            Tok::Ident(s) => {
                feed(b"i");
                feed(s.as_bytes());
            }
            Tok::Str(_) => feed(b"s"),
            Tok::Num(_) => feed(b"n"),
            Tok::Punct(c) => {
                let mut b = [0u8; 4];
                feed(b"p");
                feed(c.encode_utf8(&mut b).as_bytes());
            }
        }
        i += 1;
    }
    Some(h)
}

/// Known boilerplate shapes: (fingerprint, description).
pub const KNOWN: &[(u64, &str)] = include!("boilerplate_table.in");

/// Name of the boilerplate shape this script matches, if any.
pub fn match_boilerplate(src: &str) -> Option<&'static str> {
    let fp = fingerprint(src)?;
    KNOWN.iter().find(|(h, _)| *h == fp).map(|(_, n)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_strings_numbers_and_comments() {
        let a = fingerprint("if (typeof(x)==\"a\") x = 9.0; // hi").unwrap();
        let b = fingerprint("if(typeof(x)==\"zzz\")\n x=10; /* c */").unwrap();
        let c = fingerprint("if (typeof(y)==\"a\") y = 9.0;").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn lookalike_substring_does_not_match() {
        assert!(match_boilerplate("ADBE.Reader_Need_Version = 9.0; evil();").is_none());
    }
}
