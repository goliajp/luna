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
    /// 1-based source line where the error was detected. `0` marks a
    /// message that PUC raises without a position prefix (e.g. 5.4's
    /// "C stack overflow" while parsing, or a memory error).
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

    /// An error PUC reports with no `chunk:line:` prefix.
    pub fn unpositioned(msg: impl Into<Vec<u8>>) -> Self {
        SyntaxError {
            line: 0,
            msg: msg.into(),
        }
    }

    /// The complete message as `load` returns it: `<chunkid>:<line>: msg`,
    /// or the bare message for an unpositioned error. `chunkid` is the
    /// chunk name already rendered by `luaO_chunkid`.
    pub fn render(&self, chunkid: &[u8]) -> Vec<u8> {
        if self.line == 0 {
            return self.msg.clone();
        }
        let mut out = chunkid.to_vec();
        out.push(b':');
        out.extend_from_slice(self.line.to_string().as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(&self.msg);
        out
    }

    /// Lossy `&str` for Rust-side display (PUC `luaG_addinfo` only cares
    /// about the bytes; this is for unit tests / panic messages).
    pub fn msg_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.msg)
    }
}

/// PUC 5.1 `luaO_chunkid` at the size its lexer uses for syntax errors
/// (`MAXSRC` = 80, where 5.2+ use `LUA_IDSIZE` = 60 everywhere). The
/// algorithm differs from 5.2's too: a `\r` also ends the first line, and
/// the budget is taken as `bufflen - sizeof(" [string \"...\"] ")`.
pub fn chunk_id_51(source: &[u8]) -> Vec<u8> {
    const MAXSRC: usize = 80;
    match source.first() {
        Some(b'=') => source[1..source.len().min(MAXSRC)].to_vec(),
        Some(b'@') => {
            let s = &source[1..];
            let budget = MAXSRC - b" '...' \0".len();
            let mut out = Vec::new();
            if s.len() > budget {
                out.extend_from_slice(b"...");
                out.extend_from_slice(&s[s.len() - budget..]);
            } else {
                out.extend_from_slice(s);
            }
            out
        }
        _ => {
            let first_line = source
                .iter()
                .position(|&c| c == b'\n' || c == b'\r')
                .unwrap_or(source.len());
            let budget = MAXSRC - b" [string \"...\"] \0".len();
            let len = first_line.min(budget);
            let mut out = b"[string \"".to_vec();
            out.extend_from_slice(&source[..len]);
            if len < source.len() {
                out.extend_from_slice(b"...");
            }
            out.extend_from_slice(b"\"]");
            out
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            return f.write_str(&self.msg_str());
        }
        write!(f, "{}: {}", self.line, self.msg_str())
    }
}

impl std::error::Error for SyntaxError {}
