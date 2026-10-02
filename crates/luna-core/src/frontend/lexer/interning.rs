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
}
