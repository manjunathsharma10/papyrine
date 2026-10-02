//! Whole-script verdicts plus a coarse "why not" classification used by the
//! corpus measurement (Spike 0.4). The classification is heuristic and only
//! feeds reports; acceptance is decided by the strict recognizer alone.

use crate::boilerplate::match_boilerplate;
use crate::lexer::{Tok, tokenize};
use crate::recognize::{Call, Reject, recognize};

/// Primary reason a script is outside the native subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Blocker {
    /// Looks like Adobe's version/XFA check but is not a known fingerprint.
    UnknownBoilerplate,
    /// Submit, reset, print, mail, URL launch, page navigation, import/export.
    Navigation,
    /// Reads or writes other fields or their properties (`getField`, `this.x`).
    FieldAccess,
    /// `util.scand`, `util.printd`, `util.printf`, `util.printx`.
    UtilFunctions,
    /// Regular expressions, `.match`, `.replace`, `.test`, `.split`.
    RegExp,
    /// `app.alert`, `app.response`, `app.execMenuItem` and other `app.*`.
    AppObject,
    /// `function` definitions or calls to other user-defined names.
    CustomFunction,
    /// `if`/`for`/`while`/`switch`/`try` without a more specific marker.
    ControlFlow,
    /// Direct `event.*` manipulation (`event.value = …`, `event.rc = …`).
    EventManipulation,
    /// An `AF*` function outside the allowlist.
    AfNotAllowlisted,
    /// Allowlisted AF call with arguments the strict grammar refuses
    /// (non-literal, wrong arity, wrong kind).
    AfBadArguments,
    /// Lexing failed (regex literals, template strings, odd characters).
    LexFailure,
    /// Anything else (assignments, arithmetic, member calls).
    Other,
}

impl Blocker {
    pub fn label(self) -> &'static str {
        match self {
            Blocker::UnknownBoilerplate => "adobe-boilerplate-unrecognised",
            Blocker::Navigation => "navigation-submit-print-url",
            Blocker::FieldAccess => "field-access (getField/this.*)",
            Blocker::UtilFunctions => "util.* (scand/printd/printf)",
            Blocker::RegExp => "regexp-and-string-methods",
            Blocker::AppObject => "app.* (alert/response/other)",
            Blocker::CustomFunction => "custom-functions",
            Blocker::ControlFlow => "control-flow",
            Blocker::EventManipulation => "event.* manipulation",
            Blocker::AfNotAllowlisted => "AF-function-not-allowlisted",
            Blocker::AfBadArguments => "AF-call-bad-arguments",
            Blocker::LexFailure => "lex-failure",
            Blocker::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Whitespace/comments only.
    Empty,
    /// Pure allowlisted AF calls.
    Accepted(Vec<Call>),
    /// Known Adobe boilerplate (no-op).
    Boilerplate(&'static str),
    Rejected {
        reason: Reject,
        blocker: Blocker,
    },
}

impl Verdict {
    /// Does the native subset fully cover this script?
    pub fn covered(&self) -> bool {
        !matches!(self, Verdict::Rejected { .. })
    }
}

pub fn analyze(src: &str) -> Verdict {
    if let Some(name) = match_boilerplate(src) {
        return Verdict::Boilerplate(name);
    }
    match recognize(src) {
        Ok(calls) if calls.is_empty() => Verdict::Empty,
        Ok(calls) => Verdict::Accepted(calls),
        Err(reason) => {
            let blocker = classify(src, &reason);
            Verdict::Rejected { reason, blocker }
        }
    }
}

/// Identifier-ish words of `src`, plus `a.b` adjacency, from tokens when the
/// text lexes and from a crude character scan when it does not (regex
/// literals and the like), so unlexable scripts are still attributed.
struct Features {
    ids: Vec<String>,
    members: Vec<(String, String)>,
    lexed: bool,
}

impl Features {
    fn of(src: &str) -> Features {
        if let Ok(toks) = tokenize(src) {
            let mut ids = Vec::new();
            let mut members = Vec::new();
            for (i, t) in toks.iter().enumerate() {
                if let Tok::Ident(a) = &t.tok {
                    ids.push(a.clone());
                    if let (Some(Tok::Punct('.')), Some(Tok::Ident(b))) = (
                        toks.get(i + 1).map(|x| &x.tok),
                        toks.get(i + 2).map(|x| &x.tok),
                    ) {
                        members.push((a.clone(), b.clone()));
                    }
                }
            }
            return Features {
                ids,
                members,
                lexed: true,
            };
        }
        let mut ids = Vec::new();
        let mut members = Vec::new();
        let words: Vec<&str> = src
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$' || c == '.'))
            .filter(|w| !w.is_empty())
            .collect();
        for w in words {
            let parts: Vec<&str> = w.split('.').filter(|p| !p.is_empty()).collect();
            for (i, p) in parts.iter().enumerate() {
                ids.push((*p).to_string());
                if let Some(n) = parts.get(i + 1) {
                    members.push(((*p).to_string(), (*n).to_string()));
                }
            }
        }
        Features {
            ids,
            members,
            lexed: false,
        }
    }
    fn has(&self, w: &str) -> bool {
        self.ids.iter().any(|i| i == w)
    }
    fn member_of(&self, obj: &str) -> bool {
        self.members.iter().any(|(a, _)| a == obj)
    }
}

fn classify(src: &str, reason: &Reject) -> Blocker {
    let f = Features::of(src);
    let has = |w: &str| f.has(w);

    if has("ADBE") || has("xfa_installed") || has("xfa_version") {
        return Blocker::UnknownBoilerplate;
    }
    if [
        "submitForm",
        "resetForm",
        "print",
        "mailDoc",
        "mailForm",
        "getURL",
        "launchURL",
        "pageNum",
        "exportAsFDF",
        "importAnFDF",
        "exportAsXFDF",
        "importDataObject",
        "importXFAData",
    ]
    .iter()
    .any(|w| has(w))
    {
        return Blocker::Navigation;
    }
    if has("getField") || has("getNthFieldName") || has("numFields") {
        return Blocker::FieldAccess;
    }
    if f.member_of("util") {
        return Blocker::UtilFunctions;
    }
    if has("RegExp")
        || ["match", "replace", "test", "search", "split"]
            .iter()
            .any(|w| has(w))
    {
        return Blocker::RegExp;
    }
    if f.member_of("app") {
        return Blocker::AppObject;
    }
    match reason {
        Reject::UnknownFunction(n) if n.starts_with("AF") => return Blocker::AfNotAllowlisted,
        Reject::BadArity { .. } | Reject::BadArgument { .. } => return Blocker::AfBadArguments,
        _ => {}
    }
    if has("function") {
        return Blocker::CustomFunction;
    }
    if ["if", "else", "for", "while", "switch", "try", "do"]
        .iter()
        .any(|w| has(w))
    {
        return Blocker::ControlFlow;
    }
    if f.member_of("event") {
        return Blocker::EventManipulation;
    }
    if !f.lexed {
        return Blocker::LexFailure;
    }
    if let Reject::UnknownFunction(_) = reason {
        return Blocker::CustomFunction;
    }
    Blocker::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(analyze("  // x"), Verdict::Empty);
        assert!(matches!(
            analyze("AFDate_FormatEx('mm/dd/yyyy');"),
            Verdict::Accepted(_)
        ));
        let v = analyze("this.getField('a').value = 1;");
        assert!(matches!(
            v,
            Verdict::Rejected {
                blocker: Blocker::FieldAccess,
                ..
            }
        ));
        let v = analyze("var d = util.scand('mm/dd/yyyy', event.value);");
        assert!(matches!(
            v,
            Verdict::Rejected {
                blocker: Blocker::UtilFunctions,
                ..
            }
        ));
        let v = analyze("if (event.value > 3) event.rc = false;");
        assert!(matches!(
            v,
            Verdict::Rejected {
                blocker: Blocker::ControlFlow,
                ..
            }
        ));
        let v = analyze("AFFoo_Bar(1);");
        assert!(matches!(
            v,
            Verdict::Rejected {
                blocker: Blocker::AfNotAllowlisted,
                ..
            }
        ));
    }
}
