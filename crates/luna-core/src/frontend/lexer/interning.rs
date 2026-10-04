//! The load path's lexer, which interns identifiers (see [`Names`]).

use super::*;

impl<'s> Lexer<'s> {
    /// A lexer that interns identifiers instead of handing out their text
    /// (see [`Lexer::last_sym`]).
    pub(crate) fn interning(
        src: &'s [u8],
        version: LuaVersion,
        names: Names,
        buf: LVec<u8>,
    ) -> Result<Lexer<'s>, Oom> {
        let mut lex = Lexer::with_buf(Whole(src), version, buf, None);
        lex.names = Some(names.reuse(src.len())?);
        Ok(lex)
    }
}

impl<'f> Lexer<'f, Stream<'f>> {
    /// An interning lexer reading `src` piece by piece.
    pub(crate) fn interning_stream(
        src: Stream<'f>,
        version: LuaVersion,
        names: Names,
        buf: LVec<u8>,
    ) -> Result<Lexer<'f, Stream<'f>>, Oom> {
        let len = src.bytes().len();
        let mut lex = Lexer::with_buf(src, version, buf, None);
        lex.names = Some(names.reuse(len)?);
        Ok(lex)
    }
}

impl<S: Source> Lexer<'_, S> {
    /// The token buffer, for the next lexer to start with.
    pub(crate) fn take_buf(&mut self) -> LVec<u8> {
        self.buf.take()
    }

    /// Why the source stopped reading, when it ran out of memory.
    pub(crate) fn source_out_of_memory(&self) -> Option<Oom> {
        self.src.out_of_memory()
    }

    /// The identifiers an interning lexer has read so far.
    pub(crate) fn names(&self) -> &Names {
        self.names.as_ref().expect("an interning lexer")
    }

    /// The identifiers an interning lexer has read.
    pub(crate) fn take_names(&mut self) -> Names {
        self.names
            .take()
            .unwrap_or_else(|| Names::new(self.buf.mem()))
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
    pub(super) fn str_token(&mut self, from: usize, to: usize) -> Result<Tok, Oom> {
        match &mut self.names {
            Some(names) => self.last_sym = names.intern(&self.buf[from..to])?,
            None => self.str_range = (from, to),
        }
        Ok(Token::Str(()))
    }
}
