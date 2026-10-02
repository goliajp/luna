//! The token feed of the parser: a live lexer or a pre-expanded token
//! vector.

use super::*;

/// Token feed for the recursive-descent parser. Either a live [`Lexer`]
/// (the default `parse(src, version)` path) or a pre-materialized token
/// vector (the [`parse_tokens`] entry point used by the MacroLua expander
/// pre-pass — see `frontend::macro_expander`). Both arms support
/// `next_token` and `src()`.
pub(crate) enum TokenSource<'s> {
    /// Streaming lexer over raw source bytes.
    Lexer(Lexer<'s>),
    /// Pre-materialized token stream + a back-pointer to the original
    /// source bytes so `Token::describe` can still slice spans for
    /// `... near 'tok'` error reporting.
    PreExpanded {
        tokens: Vec<TokenInfo>,
        cursor: usize,
        src: &'s [u8],
        /// the names of the tokens read so far
        names: Names,
    },
}

/// A token as the parser holds it. `char` is set when the lexer handed back
/// a byte no token starts with; `info.tok` is then a placeholder that no
/// grammar rule accepts ([`Token::At`], which only MacroLua lexes, and
/// MacroLua never parses from a live lexer).
pub(super) struct Cur {
    pub(super) info: LexTok,
    pub(super) char: Option<u8>,
    /// the interned name of a `Token::Name` or literal of a `Token::Str`,
    /// whose text a live lexer leaves empty
    pub(super) sym: Sym,
}

impl<'s> TokenSource<'s> {
    pub(super) fn next_token(&mut self) -> Result<Cur, SyntaxError> {
        match self {
            TokenSource::Lexer(l) => Ok(match l.next_lexed()? {
                Lexed::Tok(info) => Cur {
                    info,
                    char: None,
                    sym: l.last_sym,
                },
                Lexed::Char(c, mut info) => {
                    info.tok = Token::At;
                    Cur {
                        info,
                        char: Some(c),
                        sym: Sym(0),
                    }
                }
            }),
            TokenSource::PreExpanded {
                tokens,
                cursor,
                src,
                names,
            } => {
                if *cursor >= tokens.len() {
                    let line = tokens.last().map(|t| t.line).unwrap_or(1);
                    let _ = src;
                    Ok(Cur {
                        info: LexTok {
                            tok: Token::Eof,
                            span: Span::new(0, 0),
                            line,
                        },
                        char: None,
                        sym: Sym(0),
                    })
                } else {
                    let t = &tokens[*cursor];
                    *cursor += 1;
                    let sym = match &t.tok {
                        Token::Name(text) => names.intern(text.as_bytes()),
                        Token::Str(bytes) => names.intern(bytes),
                        _ => Sym(0),
                    };
                    Ok(Cur {
                        info: LexTok {
                            tok: t.tok.kind(),
                            span: t.span,
                            line: t.line,
                        },
                        char: None,
                        sym,
                    })
                }
            }
        }
    }

    pub(super) fn names(&self) -> &Names {
        match self {
            TokenSource::Lexer(l) => l.names(),
            TokenSource::PreExpanded { names, .. } => names,
        }
    }

    pub(super) fn take_names(&mut self) -> Names {
        match self {
            TokenSource::Lexer(l) => l.take_names(),
            TokenSource::PreExpanded { names, .. } => std::mem::take(names),
        }
    }

    pub(super) fn take_buf(&mut self) -> Vec<u8> {
        match self {
            TokenSource::Lexer(l) => l.take_buf(),
            TokenSource::PreExpanded { .. } => Vec::new(),
        }
    }

    pub(super) fn src(&self) -> &'s [u8] {
        match self {
            TokenSource::Lexer(l) => l.src(),
            TokenSource::PreExpanded { src, .. } => src,
        }
    }

    /// PUC `ls->linenumber`: where the scanner stands, which is where every
    /// syntax error is reported.
    pub(super) fn line(&self) -> u32 {
        match self {
            TokenSource::Lexer(l) => l.line(),
            TokenSource::PreExpanded { tokens, cursor, .. } => tokens
                .get(cursor.saturating_sub(1))
                .or(tokens.last())
                .map_or(1, |t| t.line),
        }
    }
}
