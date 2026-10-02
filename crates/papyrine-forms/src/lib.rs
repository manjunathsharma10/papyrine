//! Native AcroForm JavaScript subset (ADR-006).
//!
//! No JavaScript engine is embedded. A script is accepted only if the strict
//! [`recognize`] grammar matches it entirely: a sequence of calls to
//! allowlisted `AF*` functions with literal arguments. Adobe's own
//! viewer-version boilerplate is recognised by fingerprint and ignored.
//! Everything else is rejected before anything runs.
//!
//! Accepted calls run through [`execute`] against an [`Event`], with Rust
//! implementations of the Acrobat functions. Zero dependencies.

pub mod boilerplate;
pub mod calc;
pub mod datefmt;
pub mod diagnose;
pub mod event;
pub mod exec;
pub mod lexer;
pub mod numfmt;
pub mod recognize;
pub mod special;

pub use calc::{FormValues, MapForm, run_calculation_order};
pub use datefmt::{DateEnv, DateTime};
pub use diagnose::{Blocker, Verdict, analyze};
pub use event::{AfError, Alert, Event, FieldLookup, TextColor, merge_change};
pub use exec::{execute, execute_call};
pub use recognize::{AfFunc, Arg, Call, Reject, recognize};

/// Convenience: recognize `script` and run it against `ev`.
/// `Err` carries either a rejection (nothing ran) or a runtime error.
pub fn run_script(
    script: &str,
    ev: &mut Event,
    fields: &dyn FieldLookup,
    env: &DateEnv,
) -> Result<(), RunError> {
    let calls = recognize(script).map_err(RunError::Rejected)?;
    execute(&calls, ev, fields, env).map_err(RunError::Af)
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Rejected(Reject),
    Af(AfError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Rejected(r) => write!(f, "rejected: {r}"),
            RunError::Af(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RunError {}
