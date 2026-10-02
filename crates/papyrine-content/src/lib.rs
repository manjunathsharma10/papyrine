//! PDF content-stream lexer, parser and serializer (ARCHITECTURE §4.2).
//!
//! The lexer and parser never panic and never fail: malformed input produces
//! the best-effort operations plus a list of [`ParseError`]s with byte offsets.
//! `parse(serialize(ops))` yields the same operations as `ops` for any `ops`
//! produced by [`parse`] (given lossless number formatting; see
//! [`SerializeOptions`]). Comments and insignificant whitespace are dropped.

pub mod builder;
pub mod lexer;
pub mod object;
pub mod parser;
pub mod serialize;

pub use builder::{ContentBuilder, TextItem};
pub use lexer::{Lexer, Token, TokenKind};
pub use object::{Object, Op, ops_equal};
pub use parser::{ParseError, ParseErrorKind, Parsed, parse};
pub use serialize::{SerializeOptions, serialize, write_object, write_op};
