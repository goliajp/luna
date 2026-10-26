//! The token feed of the parser: a live lexer or a pre-expanded token
//! vector.

use super::*;
use crate::runtime::mem::{LVec, Oom};

/// Token feed for the recursive-descent parser. Either a live [`Lexer`]
/// (the default `parse(src, version)` path) or a pre-materialized token
/// vector (the [`parse_tokens`] entry point used by the MacroLua expander
/// pre-pass — see `frontend::macro_expander`). Both arms support
/// `next_token` and `src()`.
pub(crate) enum TokenSource<'s> {
    /// Streaming lexer over raw source bytes.
    Lexer(Lexer<'s>),
    /// Lexer over a source read piece by piece (`lua_load` / `load` with a
    /// reader).
    Stream(Lexer<'s, Stream<'s>>),
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
    /// The parse opened or closed a function (see `Source::opened`).
    pub(super) fn opened(&mut self, open: bool) {
        if let TokenSource::Stream(l) = self {
            l.source_mut().opened(open);
        }
    }

    pub(super) fn next_token(&mut self) -> Result<Cur, SyntaxError> {
        match self {
            TokenSource::Lexer(l) => lexed(l),
            TokenSource::Stream(l) => lexed(l),
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
            TokenSource::Stream(l) => l.names(),
            TokenSource::PreExpanded { names, .. } => names,
        }
    }

    pub(super) fn take_names(&mut self) -> Names {
        match self {
            TokenSource::Lexer(l) => l.take_names(),
            TokenSource::Stream(l) => l.take_names(),
            TokenSource::PreExpanded { names, .. } => names.take(),
        }
    }

    /// The source a streamed parse read (kept to place a compile error).
    pub(super) fn take_streamed(&mut self) -> Option<LVec<u8>> {
        match self {
            TokenSource::Stream(l) => Some(l.source_mut().take_bytes()),
            _ => None,
        }
    }

    pub(super) fn take_buf(&mut self) -> LVec<u8> {
        match self {
            TokenSource::Lexer(l) => l.take_buf(),
            TokenSource::Stream(l) => l.take_buf(),
            TokenSource::PreExpanded { names, .. } => LVec::new(names.mem()),
        }
    }

    /// Why a streamed source stopped reading, when it ran out of memory: the
    /// parse then fails with a memory error whatever it found.
    pub(super) fn out_of_memory(&self) -> Option<Oom> {
        match self {
            TokenSource::Stream(l) => l.source_out_of_memory(),
            _ => None,
        }
    }

    pub(super) fn src(&self) -> &[u8] {
        match self {
            TokenSource::Lexer(l) => l.src(),
            TokenSource::Stream(l) => l.bytes(),
            TokenSource::PreExpanded { src, .. } => src,
        }
    }

    /// The near-token of the scanner's error, and where the token it was
    /// reading starts and where it stopped.
    pub(super) fn scan_state(&self) -> (crate::frontend::error::NearKind, usize, usize) {
        match self {
            TokenSource::Lexer(l) => (l.near_kind.get(), l.tok_start, l.pos()),
            TokenSource::Stream(l) => (l.near_kind.get(), l.tok_start, l.pos()),
            TokenSource::PreExpanded { .. } => Default::default(),
        }
    }

    /// PUC `ls->linenumber`: where the scanner stands, which is where every
    /// syntax error is reported.
    pub(super) fn line(&self) -> u32 {
        match self {
            TokenSource::Lexer(l) => l.line(),
            TokenSource::Stream(l) => l.line(),
            TokenSource::PreExpanded { tokens, cursor, .. } => tokens
                .get(cursor.saturating_sub(1))
                .or(tokens.last())
                .map_or(1, |t| t.line),
        }
    }
}

impl<'s> TokenSource<'s> {
    /// The next token written into `tok`, with the byte no token starts
    /// with and the token's name or literal; `tok` is left as it was on an
    /// error.
    #[inline]
    pub(super) fn next_into(&mut self, tok: &mut LexTok) -> Result<(Option<u8>, Sym), SyntaxError> {
        let l = match self {
            TokenSource::Lexer(l) => return lexed_into(l, tok),
            TokenSource::Stream(l) => return lexed_into(l, tok),
            TokenSource::PreExpanded { .. } => self.next_token()?,
        };
        *tok = l.info;
        Ok((l.char, l.sym))
    }
}

#[inline]
fn lexed_into<S: Source>(
    l: &mut Lexer<'_, S>,
    tok: &mut LexTok,
) -> Result<(Option<u8>, Sym), SyntaxError> {
    // a reader that failed stops the scan first, whatever it found
    let odd = l.next_into(tok);
    l.reader_failed()?;
    let odd = odd?;
    Ok(match odd {
        None => (None, l.last_sym),
        Some(c) => {
            tok.tok = Token::At;
            (Some(c), Sym(0))
        }
    })
}

/// The next item of a live lexer, as the parser holds it.
fn lexed<S: Source>(l: &mut Lexer<'_, S>) -> Result<Cur, SyntaxError> {
    let lexed = l.next_lexed();
    l.reader_failed()?;
    let lexed = lexed?;
    Ok(match lexed {
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
    })
}
