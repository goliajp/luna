use crate::frontend::span::Span;

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
    /// Human-readable form used in `near '%s'` error messages.
    pub fn describe(&self, src: &[u8], span: Span) -> String {
        match self {
            Token::Eof => "<eof>".to_string(),
            Token::Str(_) | Token::Int(_) | Token::Float(_) | Token::Name(_) => {
                String::from_utf8_lossy(span.slice(src)).into_owned()
            }
            _ => String::from_utf8_lossy(span.slice(src)).into_owned(),
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
