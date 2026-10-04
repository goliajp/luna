//! The load path's lexer, which interns identifiers (see [`Names`]).

use super::*;

impl<'s> Lexer<'s> {
    /// A lexer that interns identifiers instead of handing out their text
    /// (see [`Lexer::last_sym`]).
    pub(crate) fn interning(
        src: &'s [u8],
        version: LuaVersion,
        names: Names,
        mut buf: Vec<u8>,
    ) -> Lexer<'s> {
        buf.clear();
        Lexer {
            names: Some(names.reuse(src.len())),
            buf,
            ..Lexer::new(src, version)
        }
    }
}

impl<'f> Lexer<'f, Stream<'f>> {
    /// An interning lexer reading `src` piece by piece.
    pub(crate) fn interning_stream(
        src: Stream<'f>,
        version: LuaVersion,
        names: Names,
        mut buf: Vec<u8>,
    ) -> Lexer<'f, Stream<'f>> {
        buf.clear();
        let len = src.bytes().len();
        Lexer {
            names: Some(names.reuse(len)),
            buf,
            ..Lexer::over(src, version)
        }
    }
}

impl<S: Source> Lexer<'_, S> {
    /// The token buffer, for the next lexer to start with.
    pub(crate) fn take_buf(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    /// The identifiers an interning lexer has read so far.
    pub(crate) fn names(&self) -> &Names {
        self.names.as_ref().expect("an interning lexer")
    }

    /// The identifiers an interning lexer has read.
    pub(crate) fn take_names(&mut self) -> Names {
        self.names.take().unwrap_or_default()
    }

    /// The public form of a token just lexed by this (non-interning) lexer:
    /// a name's text is its source, a string's contents are in the buffer.
    pub(super) fn token_info(&self, t: LexTok) -> TokenInfo {
        let (from, to) = self.str_range;
        TokenInfo {
            tok: t.tok.map(
                |()| self.buf[from..to].to_vec(),
                |()| String::from_utf8_lossy(t.span.slice(self.src.bytes())).into(),
                |()| Box::default(),
            ),
            span: t.span,
            line: t.line,
        }
    }

    /// The string token of `buf[from..to]`, interned when this lexer
    /// interns (see [`Lexer::last_sym`]).
    pub(super) fn str_token(&mut self, from: usize, to: usize) -> Tok {
        match &mut self.names {
            Some(names) => self.last_sym = names.intern(&self.buf[from..to]),
            None => self.str_range = (from, to),
        }
        Token::Str(())
    }
}
