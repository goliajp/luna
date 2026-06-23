//! Syntax-error type produced by the lexer and parser.

use std::fmt;

/// Syntax error, formatted PUC-style: `chunkname:line: msg near 'tok'`.
/// The `near` part is already baked into `msg` at construction time.
///
/// `msg` is a raw byte string — PUC 5.1 reports `near '\xff'`-style errors
/// with the offending source byte verbatim, and `errors.lua` 5.1 :20 grep-
/// matches that pattern. Carrying the message as `Vec<u8>` lets the lexer
/// emit those bytes without UTF-8 enforcement getting in the way.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    /// 1-based source line where the error was detected.
    pub line: u32,
    /// Message bytes (PUC-style; may contain non-UTF-8 source bytes).
    pub msg: Vec<u8>,
}

impl SyntaxError {
    /// Build a `SyntaxError` at the given line with the given message bytes.
    pub fn new(line: u32, msg: impl Into<Vec<u8>>) -> Self {
        SyntaxError {
            line,
            msg: msg.into(),
        }
    }

    /// Lossy `&str` for Rust-side display (PUC `luaG_addinfo` only cares
    /// about the bytes; this is for unit tests / panic messages).
    pub fn msg_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.msg)
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.line, self.msg_str())
    }
}

impl std::error::Error for SyntaxError {}
