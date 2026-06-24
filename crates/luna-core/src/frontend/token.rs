//! Lexical tokens produced by the lexer.

use crate::frontend::span::Span;
use crate::version::LuaVersion;

/// One lexical token produced by the lexer.
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    // keywords
    /// `and` keyword.
    And,
    /// `break` keyword.
    Break,
    /// `do` keyword.
    Do,
    /// `else` keyword.
    Else,
    /// `elseif` keyword.
    Elseif,
    /// `end` keyword.
    End,
    /// `false` keyword.
    False,
    /// `for` keyword.
    For,
    /// `function` keyword.
    Function,
    /// 5.5 `global` keyword.
    Global,
    /// `goto` keyword.
    Goto,
    /// `if` keyword.
    If,
    /// `in` keyword.
    In,
    /// `local` keyword.
    Local,
    /// `nil` keyword.
    Nil,
    /// `not` keyword.
    Not,
    /// `or` keyword.
    Or,
    /// `repeat` keyword.
    Repeat,
    /// `return` keyword.
    Return,
    /// `then` keyword.
    Then,
    /// `true` keyword.
    True,
    /// `until` keyword.
    Until,
    /// `while` keyword.
    While,
    // symbols
    /// `+` symbol.
    Plus,
    /// `-` symbol.
    Minus,
    /// `*` symbol.
    Star,
    /// `/` symbol.
    Slash,
    /// `//` symbol (floor division).
    DSlash,
    /// `%` symbol.
    Percent,
    /// `^` symbol.
    Caret,
    /// `#` symbol.
    Hash,
    /// `&` symbol.
    Amp,
    /// `~` symbol (bitwise xor / unary bnot).
    Tilde,
    /// `|` symbol.
    Pipe,
    /// `<<` symbol.
    Shl,
    /// `>>` symbol.
    Shr,
    /// `==` symbol.
    Eq,
    /// `~=` symbol.
    Ne,
    /// `<=` symbol.
    Le,
    /// `>=` symbol.
    Ge,
    /// `<` symbol.
    Lt,
    /// `>` symbol.
    Gt,
    /// `=` symbol (assignment).
    Assign,
    /// `(` symbol.
    LParen,
    /// `)` symbol.
    RParen,
    /// `{` symbol.
    LBrace,
    /// `}` symbol.
    RBrace,
    /// `[` symbol.
    LBracket,
    /// `]` symbol.
    RBracket,
    /// `::` symbol (label delimiter).
    DColon,
    /// `;` symbol.
    Semi,
    /// `:` symbol.
    Colon,
    /// `,` symbol.
    Comma,
    /// `.` symbol.
    Dot,
    /// `..` symbol (concatenation).
    Concat,
    /// `...` symbol (vararg).
    Ellipsis,
    // literals
    /// Integer literal.
    Int(
        /// Decoded 64-bit signed value.
        i64,
    ),
    /// Floating-point literal.
    Float(
        /// Decoded IEEE-754 double value.
        f64,
    ),
    /// String literal (raw bytes; Lua strings are 8-bit clean).
    Str(
        /// Decoded byte contents.
        Vec<u8>,
    ),
    /// Identifier.
    Name(
        /// Source text of the identifier.
        Box<str>,
    ),
    /// MacroLua `@` sigil (v1.3 Phase ML). Lexed only when
    /// `version.is_macro_lua()`; PUC 5.1-5.5 sources continue to
    /// error `unexpected symbol near '@'`.
    At,
    /// MacroLua explicit quote-block opener `@{`. Lexed only when
    /// `version.is_macro_lua()`; pairs with [`Token::MacroBraceClose`].
    MacroBraceOpen,
    /// MacroLua explicit quote-block closer `}@`. Lexed only when
    /// `version.is_macro_lua()`; pairs with [`Token::MacroBraceOpen`].
    MacroBraceClose,
    /// Synthetic token produced by the macro expander pre-pass: a
    /// captured token run (the body of a `@quote{...}` or `@{...}@`
    /// block). The lexer never emits this. After the expander runs
    /// it splices these back into the stream as raw token sequences
    /// before the parser proper sees them.
    MacroQuote(
        /// Captured token run.
        Box<[TokenInfo]>,
    ),
    /// End-of-file marker.
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
#[derive(Clone, Debug, PartialEq)]
pub struct TokenInfo {
    /// The lexical token.
    pub tok: Token,
    /// Byte range in source.
    pub span: Span,
    /// 1-based source line where the token starts.
    pub line: u32,
}
