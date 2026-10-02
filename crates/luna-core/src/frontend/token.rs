//! Lexical tokens produced by the lexer.

use crate::frontend::span::Span;
use crate::version::LuaVersion;

/// One lexical token produced by the lexer.
///
/// The type parameters are the payloads of [`Token::Str`], [`Token::Name`]
/// and [`Token::MacroQuote`]; every token handed out uses the defaults.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Token<S = Vec<u8>, N = Box<str>, Q = Box<[TokenInfo]>> {
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
        S,
    ),
    /// Identifier.
    Name(
        /// Source text of the identifier.
        N,
    ),
    /// MacroLua `@` sigil. Lexed only when
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
        Q,
    ),
    /// End-of-file marker.
    Eof,
}

impl Token {
    /// The near-token shown in `... near <tok>` error messages, rendered the
    /// way the dialect's `txtToken` / `luaX_token2str` does (see
    /// `near_text`). Lossy view of `Token::near_bytes`.
    pub fn describe(&self, src: &[u8], span: Span, version: LuaVersion) -> String {
        String::from_utf8_lossy(&self.near_bytes(src, span, version)).into_owned()
    }

    /// Raw-byte form of [`Token::describe`]: names, strings and numerals
    /// are shown through PUC's lex buffer, which holds the token's text as
    /// scanned (a string keeps its delimiters and holds its *decoded*
    /// contents), so non-UTF-8 bytes survive.
    pub(crate) fn near_bytes(&self, src: &[u8], span: Span, version: LuaVersion) -> Vec<u8> {
        match self {
            Token::Eof => near_text(version, Near::Eof),
            Token::Str(content) => {
                let raw = span.slice(src);
                let mut text = Vec::with_capacity(content.len() + 4);
                if raw.first() == Some(&b'[') {
                    let level = raw[1..].iter().take_while(|&&c| c == b'=').count();
                    text.push(b'[');
                    text.extend(std::iter::repeat_n(b'=', level));
                    text.push(b'[');
                    text.extend_from_slice(content);
                    text.push(b']');
                    text.extend(std::iter::repeat_n(b'=', level));
                    text.push(b']');
                } else {
                    text.push(raw[0]);
                    text.extend_from_slice(content);
                    text.push(raw[0]);
                }
                near_text(version, Near::Text(&text))
            }
            Token::Name(_) | Token::Int(_) | Token::Float(_) => {
                near_text(version, Near::Text(span.slice(src)))
            }
            _ => near_text(version, Near::Fixed(span.slice(src))),
        }
    }
}

/// What an error's `near` part refers to, in PUC's terms.
#[derive(Clone, Copy)]
pub(crate) enum Near<'a> {
    /// `TK_EOS`.
    Eof,
    /// A single-byte token the lexer returned as itself (PUC returns every
    /// byte it does not recognise that way, so the parser reports it).
    Char(u8),
    /// A reserved word or a symbol.
    Fixed(&'a [u8]),
    /// The lex buffer (names, strings, numerals, and lexer errors inside
    /// them).
    Text(&'a [u8]),
}

/// Render a near-token per dialect. 5.1 wraps whatever `luaX_token2str`
/// produced in quotes (so `'<eof>'`, `'char(1)'`); 5.2+ quote only concrete
/// tokens and leave the `<eof>` pseudo-token bare. A non-printable byte is
/// `char(N)` up to 5.2 (5.1 decides by `iscntrl`, so a high byte goes out
/// raw) and `'<\N>'` from 5.3.
pub(crate) fn near_text(version: LuaVersion, near: Near<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    let quoted = |out: &mut Vec<u8>, s: &[u8]| {
        out.push(b'\'');
        out.extend_from_slice(s);
        out.push(b'\'');
    };
    if version <= LuaVersion::Lua51 {
        let inner: Vec<u8> = match near {
            Near::Eof => b"<eof>".to_vec(),
            Near::Char(c) if c.is_ascii_control() => format!("char({c})").into_bytes(),
            Near::Char(c) => vec![c],
            Near::Fixed(s) | Near::Text(s) => c_str(s).to_vec(),
        };
        quoted(&mut out, &inner);
        return out;
    }
    match near {
        Near::Eof => out.extend_from_slice(b"<eof>"),
        Near::Char(c) if (0x20..0x7f).contains(&c) => quoted(&mut out, &[c]),
        Near::Char(c) if version == LuaVersion::Lua52 => {
            out.extend_from_slice(format!("char({c})").as_bytes())
        }
        Near::Char(c) => quoted(&mut out, format!("<\\{c}>").as_bytes()),
        Near::Fixed(s) | Near::Text(s) => quoted(&mut out, c_str(s)),
    }
    out
}

/// PUC splices the lex buffer in with `%s`, so a NUL inside a string
/// literal ends the near-token there.
fn c_str(s: &[u8]) -> &[u8] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
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

/// A token as the lexer and parser pass it around inside the crate: the
/// payloads of strings and names live elsewhere (the lex buffer, the
/// source, or the chunk's interned names), so it is `Copy`.
pub(crate) type Tok = Token<(), (), ()>;

impl<S, N, Q> Token<S, N, Q> {
    /// This token with its payloads replaced.
    pub(crate) fn map<S2, N2, Q2>(
        self,
        s: impl FnOnce(S) -> S2,
        n: impl FnOnce(N) -> N2,
        q: impl FnOnce(Q) -> Q2,
    ) -> Token<S2, N2, Q2> {
        match self {
            Token::And => Token::And,
            Token::Break => Token::Break,
            Token::Do => Token::Do,
            Token::Else => Token::Else,
            Token::Elseif => Token::Elseif,
            Token::End => Token::End,
            Token::False => Token::False,
            Token::For => Token::For,
            Token::Function => Token::Function,
            Token::Global => Token::Global,
            Token::Goto => Token::Goto,
            Token::If => Token::If,
            Token::In => Token::In,
            Token::Local => Token::Local,
            Token::Nil => Token::Nil,
            Token::Not => Token::Not,
            Token::Or => Token::Or,
            Token::Repeat => Token::Repeat,
            Token::Return => Token::Return,
            Token::Then => Token::Then,
            Token::True => Token::True,
            Token::Until => Token::Until,
            Token::While => Token::While,
            Token::Plus => Token::Plus,
            Token::Minus => Token::Minus,
            Token::Star => Token::Star,
            Token::Slash => Token::Slash,
            Token::DSlash => Token::DSlash,
            Token::Percent => Token::Percent,
            Token::Caret => Token::Caret,
            Token::Hash => Token::Hash,
            Token::Amp => Token::Amp,
            Token::Tilde => Token::Tilde,
            Token::Pipe => Token::Pipe,
            Token::Shl => Token::Shl,
            Token::Shr => Token::Shr,
            Token::Eq => Token::Eq,
            Token::Ne => Token::Ne,
            Token::Le => Token::Le,
            Token::Ge => Token::Ge,
            Token::Lt => Token::Lt,
            Token::Gt => Token::Gt,
            Token::Assign => Token::Assign,
            Token::LParen => Token::LParen,
            Token::RParen => Token::RParen,
            Token::LBrace => Token::LBrace,
            Token::RBrace => Token::RBrace,
            Token::LBracket => Token::LBracket,
            Token::RBracket => Token::RBracket,
            Token::DColon => Token::DColon,
            Token::Semi => Token::Semi,
            Token::Colon => Token::Colon,
            Token::Comma => Token::Comma,
            Token::Dot => Token::Dot,
            Token::Concat => Token::Concat,
            Token::Ellipsis => Token::Ellipsis,
            Token::At => Token::At,
            Token::MacroBraceOpen => Token::MacroBraceOpen,
            Token::MacroBraceClose => Token::MacroBraceClose,
            Token::Eof => Token::Eof,
            Token::Int(v) => Token::Int(v),
            Token::Float(v) => Token::Float(v),
            Token::Str(v) => Token::Str(s(v)),
            Token::Name(v) => Token::Name(n(v)),
            Token::MacroQuote(v) => Token::MacroQuote(q(v)),
        }
    }

    /// The token's kind, without its payloads.
    pub(crate) fn kind(&self) -> Tok {
        match self {
            Token::And => Token::And,
            Token::Break => Token::Break,
            Token::Do => Token::Do,
            Token::Else => Token::Else,
            Token::Elseif => Token::Elseif,
            Token::End => Token::End,
            Token::False => Token::False,
            Token::For => Token::For,
            Token::Function => Token::Function,
            Token::Global => Token::Global,
            Token::Goto => Token::Goto,
            Token::If => Token::If,
            Token::In => Token::In,
            Token::Local => Token::Local,
            Token::Nil => Token::Nil,
            Token::Not => Token::Not,
            Token::Or => Token::Or,
            Token::Repeat => Token::Repeat,
            Token::Return => Token::Return,
            Token::Then => Token::Then,
            Token::True => Token::True,
            Token::Until => Token::Until,
            Token::While => Token::While,
            Token::Plus => Token::Plus,
            Token::Minus => Token::Minus,
            Token::Star => Token::Star,
            Token::Slash => Token::Slash,
            Token::DSlash => Token::DSlash,
            Token::Percent => Token::Percent,
            Token::Caret => Token::Caret,
            Token::Hash => Token::Hash,
            Token::Amp => Token::Amp,
            Token::Tilde => Token::Tilde,
            Token::Pipe => Token::Pipe,
            Token::Shl => Token::Shl,
            Token::Shr => Token::Shr,
            Token::Eq => Token::Eq,
            Token::Ne => Token::Ne,
            Token::Le => Token::Le,
            Token::Ge => Token::Ge,
            Token::Lt => Token::Lt,
            Token::Gt => Token::Gt,
            Token::Assign => Token::Assign,
            Token::LParen => Token::LParen,
            Token::RParen => Token::RParen,
            Token::LBrace => Token::LBrace,
            Token::RBrace => Token::RBrace,
            Token::LBracket => Token::LBracket,
            Token::RBracket => Token::RBracket,
            Token::DColon => Token::DColon,
            Token::Semi => Token::Semi,
            Token::Colon => Token::Colon,
            Token::Comma => Token::Comma,
            Token::Dot => Token::Dot,
            Token::Concat => Token::Concat,
            Token::Ellipsis => Token::Ellipsis,
            Token::At => Token::At,
            Token::MacroBraceOpen => Token::MacroBraceOpen,
            Token::MacroBraceClose => Token::MacroBraceClose,
            Token::Eof => Token::Eof,
            Token::Int(v) => Token::Int(*v),
            Token::Float(v) => Token::Float(*v),
            Token::Str(_) => Token::Str(()),
            Token::Name(_) => Token::Name(()),
            Token::MacroQuote(_) => Token::MacroQuote(()),
        }
    }
}

/// A [`Tok`] plus where it came from (the crate's [`TokenInfo`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LexTok {
    pub(crate) tok: Tok,
    pub(crate) span: Span,
    pub(crate) line: u32,
}
