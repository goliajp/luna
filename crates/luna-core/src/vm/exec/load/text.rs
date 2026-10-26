//! A text chunk parsed piece by piece from a reader.

use super::*;
use crate::frontend::lexer::Fed;

/// A text chunk being loaded from a reader (see `Vm::text_load`).
pub struct TextLoad {
    pub(super) version: crate::version::LuaVersion,
    pub(super) depth: u32,
    pub(super) budget: usize,
    pub(super) scratch: crate::frontend::parser::ParseScratch,
}

/// The outcome of `TextLoad::parse`, for `Vm::load_parsed`.
pub struct ParsedText(pub(super) Result<crate::frontend::parser::Parsed, Failure>);

impl ParsedText {
    /// Whether the parse stopped because its reader failed, where it was
    /// (see [`TextLoad::parse_open`]).
    pub(crate) fn reader_failed(&self) -> bool {
        matches!(&self.0, Err((e, Some(_))) if e.is_reader_failed())
    }
}

/// A parse error, with where the parse failed and the source it read.
pub(super) type Failure = (
    SyntaxError,
    Option<(crate::frontend::parser::Failed, LVec<u8>)>,
);

impl TextLoad {
    /// Parse the chunk whose first piece is `first`, calling `feed` for
    /// each further piece only when the parser moves past the end of what
    /// it has; `feed` appends a piece and returns true, or returns false at
    /// the end. Input past the loader's byte budget fails the load with
    /// "not enough memory", as [`Vm::load`] does.
    pub fn parse(self, first: Vec<u8>, feed: &mut dyn FnMut(&mut Vec<u8>) -> bool) -> ParsedText {
        self.parse_open(first, &mut |buf, _, _| match feed(buf) {
            true => Fed::More,
            false => Fed::End,
        })
    }

    /// [`TextLoad::parse`], `feed` told as well how many functions the
    /// parse has open (main included) and shown the source read so far,
    /// and able to fail: the scan stops where it was, and the parse fails.
    pub(crate) fn parse_open(
        self,
        first: Vec<u8>,
        feed: &mut dyn FnMut(&mut Vec<u8>, u32, &[u8]) -> Fed,
    ) -> ParsedText {
        let oom = || SyntaxError {
            line: 0,
            msg: b"not enough memory".to_vec(),
        };
        let budget = self.budget;
        if first.len() > budget {
            return ParsedText(Err((oom(), None)));
        }
        // the source read so far lives in the vm's memory; each piece comes
        // through a buffer of its own first, as the reader hands it over
        let mut whole = LVec::new(self.scratch.mem());
        if let Err(e) = whole.extend_from_slice(&first) {
            return ParsedText(Err((e.into(), None)));
        }
        drop(first);
        let mut over = false;
        let mut piece = Vec::new();
        let mut capped = |buf: &mut LVec<u8>, open: u32| -> Result<Fed, Oom> {
            piece.clear();
            // the reader runs Lua code, outside the load's unwinding
            let more = outside_load(|| feed(&mut piece, open, &buf[..]));
            if buf.len() + piece.len() > budget {
                over = true;
                return Ok(Fed::End);
            }
            buf.extend_from_slice(&piece)?;
            Ok(more)
        };
        let mem = self.scratch.mem();
        let (version, depth, scratch) = (self.version, self.depth, self.scratch);
        let r = catch_load_oom(|| {
            crate::frontend::parser::parse_stream(whole, &mut capped, version, depth, scratch)
        });
        let r = match r {
            Ok(r) => r,
            Err(()) => {
                mem.ctx().raise_oom();
                Err((SyntaxError::memory(), None))
            }
        };
        ParsedText(if over { Err((oom(), None)) } else { r })
    }
}
