use crate::frontend::span::Span;
use crate::version::LuaVersion;

#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    // keywords
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    Global,
    Goto,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    // symbols
    Plus,
    Minus,
    Star,
    Slash,
    DSlash,
    Percent,
    Caret,
    Hash,
    Amp,
    Tilde,
    Pipe,
    Shl,
    Shr,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    DColon,
    Semi,
    Colon,
    Comma,
    Dot,
    Concat,
    Ellipsis,
    // literals
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Name(Box<str>),
    Eof,
}

impl Token {
    /// The near-token shown in `... near <tok>` error messages. Every
    /// concrete token (names, literals, symbols, keywords) is wrapped in
    /// single quotes per PUC's `txtToken`/`luaX_token2str`. The
    /// pseudo-token `<eof>` is unquoted under 5.2+ — those suites'
    /// `checksyntax` has a `^<%a` guard that *adds* quotes only when the
    /// expected token doesn't already start with `<`. 5.1's `checksyntax`
    /// has no such guard and unconditionally wraps the expected token, so
    /// `<eof>` must come through quoted there to match the
    /// `... near '<eof>'` shape (5.1 errors.lua :20-:21 pin this; PUC's
    /// 5.1 luaX_lexerror output is the same).
    pub fn describe(&self, src: &[u8], span: Span, version: LuaVersion) -> String {
        match self {
            Token::Eof if version <= LuaVersion::Lua51 => "'<eof>'".to_string(),
            Token::Eof => "<eof>".to_string(),
            _ => format!("'{}'", String::from_utf8_lossy(span.slice(src))),
        }
    }
}

/// A token plus where it came from.
#[derive(Clone, Debug)]
pub struct TokenInfo {
    pub tok: Token,
    pub span: Span,
    pub line: u32,
}
