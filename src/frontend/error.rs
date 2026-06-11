use std::fmt;

/// Syntax error, formatted PUC-style: `chunkname:line: msg near 'tok'`.
/// The `near` part is already baked into `msg` at construction time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    pub line: u32,
    pub msg: String,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.line, self.msg)
    }
}

impl std::error::Error for SyntaxError {}
