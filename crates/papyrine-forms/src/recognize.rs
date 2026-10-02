//! Strict recognizer: a script is accepted only if it is a sequence of calls
//! to allowlisted `AF*` functions with literal arguments. Anything else is
//! rejected whole; nothing is ever partially executed.

use crate::lexer::{LexError, Tok, Token, tokenize};

/// The allowlisted Acrobat built-ins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AfFunc {
    NumberFormat,
    NumberKeystroke,
    PercentFormat,
    PercentKeystroke,
    DateFormat,
    DateFormatEx,
    DateKeystroke,
    DateKeystrokeEx,
    TimeFormat,
    TimeFormatEx,
    TimeKeystroke,
    TimeKeystrokeEx,
    SpecialFormat,
    SpecialKeystroke,
    SpecialKeystrokeEx,
    SimpleCalculate,
    RangeValidate,
}

/// A literal argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<Arg>),
}

impl Arg {
    /// JS-style truthiness of a literal.
    pub fn truthy(&self) -> bool {
        match self {
            Arg::Str(s) => !s.is_empty(),
            Arg::Num(n) => *n != 0.0 && !n.is_nan(),
            Arg::Bool(b) => *b,
            Arg::List(_) => true,
        }
    }

    /// JS-style number coercion of a scalar literal.
    pub fn to_number(&self) -> f64 {
        match self {
            Arg::Num(n) => *n,
            Arg::Bool(b) => f64::from(u8::from(*b)),
            Arg::Str(s) => crate::numfmt::js_to_number(s),
            Arg::List(_) => f64::NAN,
        }
    }

    /// Integer coercion as `ToInt32` would give, saturating (no UB, no panic).
    pub fn to_int(&self) -> i64 {
        let n = self.to_number();
        if n.is_nan() {
            0
        } else {
            n.trunc().clamp(-1e12, 1e12) as i64
        }
    }

    pub fn to_js_string(&self) -> String {
        match self {
            Arg::Str(s) => s.clone(),
            Arg::Num(n) => crate::numfmt::js_number_to_string(*n),
            Arg::Bool(b) => b.to_string(),
            Arg::List(v) => v
                .iter()
                .map(Arg::to_js_string)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// One recognized statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub func: AfFunc,
    pub args: Vec<Arg>,
}

/// Why a script was rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum Reject {
    Lex(LexError),
    /// A call to something not on the allowlist.
    UnknownFunction(String),
    /// A statement that is not `name(args)`: control flow, assignment,
    /// declarations, member access, and so on.
    NotACall {
        pos: usize,
    },
    /// Allowlisted function with a disallowed number of arguments.
    BadArity {
        func: &'static str,
        got: usize,
    },
    /// Allowlisted function with an argument of the wrong literal kind.
    BadArgument {
        func: &'static str,
        index: usize,
    },
    /// Malformed or non-literal argument list, missing terminator, etc.
    Syntax {
        pos: usize,
    },
    TooComplex,
}

impl std::fmt::Display for Reject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reject::Lex(e) => write!(f, "lex: {e}"),
            Reject::UnknownFunction(n) => write!(f, "call to non-allowlisted function {n}"),
            Reject::NotACall { pos } => write!(f, "statement at {pos} is not an AF call"),
            Reject::BadArity { func, got } => write!(f, "{func}: unsupported arity {got}"),
            Reject::BadArgument { func, index } => write!(f, "{func}: bad argument {index}"),
            Reject::Syntax { pos } => write!(f, "syntax outside the allowed grammar at {pos}"),
            Reject::TooComplex => write!(f, "too many statements or too deep"),
        }
    }
}

impl std::error::Error for Reject {}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Number (booleans are accepted and coerced, as Acrobat does).
    N,
    S,
    /// Boolean (numbers accepted and coerced).
    B,
    /// A list of strings, or one comma-separated string.
    L,
}

struct Sig {
    name: &'static str,
    func: AfFunc,
    min: usize,
    kinds: &'static [Kind],
}

use Kind::{B, L, N, S};

const SIGS: &[Sig] = &[
    Sig {
        name: "AFNumber_Format",
        func: AfFunc::NumberFormat,
        min: 5,
        kinds: &[N, N, N, N, S, B],
    },
    Sig {
        name: "AFNumber_Keystroke",
        func: AfFunc::NumberKeystroke,
        min: 5,
        kinds: &[N, N, N, N, S, B],
    },
    Sig {
        name: "AFPercent_Format",
        func: AfFunc::PercentFormat,
        min: 2,
        kinds: &[N, N, B],
    },
    Sig {
        name: "AFPercent_Keystroke",
        func: AfFunc::PercentKeystroke,
        min: 2,
        kinds: &[N, N, B],
    },
    Sig {
        name: "AFDate_Format",
        func: AfFunc::DateFormat,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFDate_FormatEx",
        func: AfFunc::DateFormatEx,
        min: 1,
        kinds: &[S],
    },
    Sig {
        name: "AFDate_Keystroke",
        func: AfFunc::DateKeystroke,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFDate_KeystrokeEx",
        func: AfFunc::DateKeystrokeEx,
        min: 1,
        kinds: &[S],
    },
    Sig {
        name: "AFTime_Format",
        func: AfFunc::TimeFormat,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFTime_FormatEx",
        func: AfFunc::TimeFormatEx,
        min: 1,
        kinds: &[S],
    },
    Sig {
        name: "AFTime_Keystroke",
        func: AfFunc::TimeKeystroke,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFTime_KeystrokeEx",
        func: AfFunc::TimeKeystrokeEx,
        min: 1,
        kinds: &[S],
    },
    Sig {
        name: "AFSpecial_Format",
        func: AfFunc::SpecialFormat,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFSpecial_Keystroke",
        func: AfFunc::SpecialKeystroke,
        min: 1,
        kinds: &[N],
    },
    Sig {
        name: "AFSpecial_KeystrokeEx",
        func: AfFunc::SpecialKeystrokeEx,
        min: 1,
        kinds: &[S],
    },
    Sig {
        name: "AFSimple_Calculate",
        func: AfFunc::SimpleCalculate,
        min: 2,
        kinds: &[S, L],
    },
    Sig {
        name: "AFRange_Validate",
        func: AfFunc::RangeValidate,
        min: 4,
        kinds: &[B, N, B, N],
    },
];

/// Look up the signature for an identifier.
pub fn lookup(name: &str) -> Option<AfFunc> {
    SIGS.iter().find(|s| s.name == name).map(|s| s.func)
}

pub fn func_name(f: AfFunc) -> &'static str {
    SIGS.iter()
        .find(|s| s.func == f)
        .map(|s| s.name)
        .unwrap_or("?")
}

const MAX_STATEMENTS: usize = 256;
const MAX_ARGS: usize = 1024;
const MAX_DEPTH: usize = 4;

struct P<'a> {
    t: &'a [Token],
    i: usize,
    args_seen: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<&Token> {
        self.t.get(self.i)
    }
    fn pos(&self) -> usize {
        self.t.get(self.i).map(|t| t.pos).unwrap_or(usize::MAX)
    }
    fn punct(&mut self, c: char) -> bool {
        if matches!(self.peek(), Some(Token { tok: Tok::Punct(p), .. }) if *p == c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn syntax<T>(&self) -> Result<T, Reject> {
        Err(Reject::Syntax { pos: self.pos() })
    }

    fn arg(&mut self, depth: usize) -> Result<Arg, Reject> {
        self.args_seen += 1;
        if self.args_seen > MAX_ARGS || depth > MAX_DEPTH {
            return Err(Reject::TooComplex);
        }
        let Some(tok) = self.peek().map(|t| t.tok.clone()) else {
            return self.syntax();
        };
        match tok {
            Tok::Str(s) => {
                self.i += 1;
                Ok(Arg::Str(s))
            }
            Tok::Num(n) => {
                self.i += 1;
                Ok(Arg::Num(n))
            }
            Tok::Punct(c @ ('-' | '+')) => {
                self.i += 1;
                match self.peek().map(|t| &t.tok) {
                    Some(Tok::Num(n)) => {
                        let n = *n;
                        self.i += 1;
                        Ok(Arg::Num(if c == '-' { -n } else { n }))
                    }
                    _ => self.syntax(),
                }
            }
            Tok::Ident(id) if id == "true" || id == "false" => {
                self.i += 1;
                Ok(Arg::Bool(id == "true"))
            }
            Tok::Punct('[') => {
                self.i += 1;
                self.list(']', depth)
            }
            Tok::Ident(id) if id == "new" || id == "Array" => {
                self.i += 1;
                if id == "new" {
                    match self.peek().map(|t| &t.tok) {
                        Some(Tok::Ident(a)) if a == "Array" => self.i += 1,
                        _ => return self.syntax(),
                    }
                }
                if !self.punct('(') {
                    return self.syntax();
                }
                self.list(')', depth)
            }
            _ => self.syntax(),
        }
    }

    fn list(&mut self, close: char, depth: usize) -> Result<Arg, Reject> {
        let mut v = Vec::new();
        if self.punct(close) {
            return Ok(Arg::List(v));
        }
        loop {
            v.push(self.arg(depth + 1)?);
            if self.punct(',') {
                continue;
            }
            if self.punct(close) {
                return Ok(Arg::List(v));
            }
            return self.syntax();
        }
    }
}

fn kind_ok(k: Kind, a: &Arg) -> bool {
    match k {
        Kind::N | Kind::B => matches!(a, Arg::Num(_) | Arg::Bool(_)),
        Kind::S => matches!(a, Arg::Str(_)),
        Kind::L => match a {
            Arg::Str(_) => true,
            Arg::List(v) => v.iter().all(|e| matches!(e, Arg::Str(_))),
            _ => false,
        },
    }
}

/// Recognize a script. An empty (or comment-only) script is valid and yields
/// no calls.
pub fn recognize(src: &str) -> Result<Vec<Call>, Reject> {
    let toks = tokenize(src).map_err(Reject::Lex)?;
    recognize_tokens(&toks)
}

pub(crate) fn recognize_tokens(toks: &[Token]) -> Result<Vec<Call>, Reject> {
    let mut p = P {
        t: toks,
        i: 0,
        args_seen: 0,
    };
    let mut calls = Vec::new();
    loop {
        while p.punct(';') {}
        let Some(first) = p.peek() else { break };
        if calls.len() >= MAX_STATEMENTS {
            return Err(Reject::TooComplex);
        }
        let start_pos = first.pos;
        let Tok::Ident(name) = first.tok.clone() else {
            return Err(Reject::NotACall { pos: start_pos });
        };
        p.i += 1;
        if !matches!(
            p.peek(),
            Some(Token {
                tok: Tok::Punct('('),
                ..
            })
        ) {
            return Err(Reject::NotACall { pos: start_pos });
        }
        let Some(sig) = SIGS.iter().find(|s| s.name == name) else {
            return Err(Reject::UnknownFunction(name));
        };
        p.i += 1;
        let Arg::List(args) = p.list(')', 0)? else {
            return p.syntax();
        };
        // Statement end: `;`, a line break, or end of script.
        let ended = p.peek().is_none_or(|t| t.nl_before)
            || matches!(
                p.peek(),
                Some(Token {
                    tok: Tok::Punct(';'),
                    ..
                })
            );
        if !ended {
            return Err(Reject::NotACall { pos: p.pos() });
        }
        if args.len() < sig.min || args.len() > sig.kinds.len() {
            return Err(Reject::BadArity {
                func: sig.name,
                got: args.len(),
            });
        }
        for (index, (a, k)) in args.iter().zip(sig.kinds).enumerate() {
            if !kind_ok(*k, a) {
                return Err(Reject::BadArgument {
                    func: sig.name,
                    index,
                });
            }
        }
        calls.push(Call {
            func: sig.func,
            args,
        });
    }
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_basic() {
        let c = recognize("AFNumber_Format(2, 0, 0, 0, \"$\", true);\nAFSimple_Calculate(\"SUM\", new Array(\"a\",\"b\"))").unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].func, AfFunc::NumberFormat);
        assert_eq!(
            c[1].args[1],
            Arg::List(vec![Arg::Str("a".into()), Arg::Str("b".into())])
        );
    }

    #[test]
    fn empty_and_comments() {
        assert!(recognize("").unwrap().is_empty());
        assert!(recognize("  // nothing\n/* x */ ;;").unwrap().is_empty());
    }

    #[test]
    fn rejects_everything_else() {
        for s in [
            "var x = 1;",
            "if (event.value) AFNumber_Format(2,0,0,0,'',true);",
            "AFNumber_Format(2,0,0,0,'',true); app.alert('x')",
            "this.getField('a').value = 1;",
            "AFNumber_Format(2,0,0,0,'',true) AFNumber_Format(2,0,0,0,'',true)",
            "AFNumber_Format(n,0,0,0,'',true);",
            "AFNumber_Format(1+1,0,0,0,'',true);",
            "AFNumber_Format(2,0,0,0);",
            "AFNumber_Format('2',0,0,0,'',true);",
            "AFDate_FormatEx(1);",
            "AFSimple_Calculate('SUM', [1,2]);",
            "evil();",
            "AFNumber_Format(2,0,0,0,'',true",
            "`x`",
        ] {
            assert!(recognize(s).is_err(), "should reject: {s}");
        }
    }

    #[test]
    fn asi_newline_separated() {
        let c = recognize("AFDate_FormatEx('mm/dd/yyyy')\nAFTime_Format(0)").unwrap();
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn negative_and_bool_args() {
        let c = recognize("AFRange_Validate(true, -5, false, +10)").unwrap();
        assert_eq!(c[0].args[1], Arg::Num(-5.0));
    }
}
