//! Where a lexer's bytes come from: a whole source in memory, or a reader
//! asked for the next piece as the scan moves past the end of what it has
//! (PUC's `ZIO`), so a syntax error stops the reading where PUC stops it.

use crate::runtime::mem::{LVec, Oom};

/// The bytes a [`super::Lexer`] scans.
pub trait Source {
    /// Whether [`Source::more`] can add bytes; a whole source never asks.
    const STREAMS: bool = false;
    /// The bytes read so far.
    fn bytes(&self) -> &[u8];
    /// Read the next piece onto the end; false at the end of the input.
    fn more(&mut self) -> bool {
        false
    }
    /// Why reading stopped, when the bytes read could not be kept.
    fn out_of_memory(&self) -> Option<Oom> {
        None
    }
    /// Whether reading stopped because the reader failed (raised an error
    /// or returned what is no piece): the scan stops where it was.
    fn reader_failed(&self) -> bool {
        false
    }
    /// The parse opened (`true`) or closed a function, where PUC's
    /// `open_func` / `close_func` run.
    fn opened(&mut self, _open: bool) {}
}

/// A source held whole in memory.
#[derive(Clone, Copy)]
pub struct Whole<'s>(pub(crate) &'s [u8]);

impl Source for Whole<'_> {
    #[inline(always)]
    fn bytes(&self) -> &[u8] {
        self.0
    }
}

/// What a reader did when asked for the next piece.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fed {
    /// appended a piece
    More,
    /// signalled the end of the input (PUC's `lua_Reader` returning no
    /// bytes)
    End,
    /// failed: PUC's reader raised an error where the scanner called it
    Failed,
}

/// A reader of the next piece: it appends one piece to the buffer, or
/// tells the end of the input or its failure; an error when the buffer
/// cannot grow. It is told how many functions the parse has open, PUC's
/// `FuncState`s (main included), whose objects PUC holds on the stack
/// meanwhile.
pub(crate) type Feed<'f> = dyn FnMut(&mut LVec<u8>, u32) -> Result<Fed, Oom> + 'f;

/// A source read piece by piece. Once the reader has signalled the end it
/// is not called again: PUC's scanner stops at the end of input.
pub(crate) struct Stream<'f> {
    buf: LVec<u8>,
    feed: &'f mut Feed<'f>,
    ended: bool,
    /// reading stopped because `buf` could not grow
    oom: Option<Oom>,
    /// reading stopped because the reader failed
    failed: bool,
    /// the functions the parse has open
    open: u32,
}

impl<'f> Stream<'f> {
    /// A stream whose first piece, `first`, was already read (to tell text
    /// from a binary chunk), with `feed` for the rest.
    pub(crate) fn new(first: LVec<u8>, feed: &'f mut Feed<'f>) -> Stream<'f> {
        let ended = first.is_empty();
        Stream {
            buf: first,
            feed,
            ended,
            oom: None,
            failed: false,
            open: 1,
        }
    }
}

impl<'f> super::Lexer<'_, Stream<'f>> {
    /// The stream the lexer reads.
    pub(crate) fn source_mut(&mut self) -> &mut Stream<'f> {
        &mut self.src
    }
}

impl Stream<'_> {
    /// The source read, the whole of it once the scan reached its end.
    pub(crate) fn take_bytes(&mut self) -> LVec<u8> {
        self.buf.take()
    }
}

impl Source for Stream<'_> {
    const STREAMS: bool = true;

    fn bytes(&self) -> &[u8] {
        &self.buf
    }

    fn more(&mut self) -> bool {
        if self.ended {
            return false;
        }
        let before = self.buf.len();
        match (self.feed)(&mut self.buf, self.open) {
            Ok(Fed::More) if self.buf.len() > before => {}
            Ok(Fed::Failed) => {
                self.ended = true;
                self.failed = true;
            }
            Ok(_) => self.ended = true,
            // the scan sees the end of the input; the parse then reports
            // the memory error instead of what it found there
            Err(e) => {
                self.buf.truncate(before);
                self.ended = true;
                self.oom = Some(e);
            }
        }
        self.buf.len() > before
    }

    fn out_of_memory(&self) -> Option<Oom> {
        self.oom
    }

    fn reader_failed(&self) -> bool {
        self.failed
    }

    fn opened(&mut self, open: bool) {
        if open {
            self.open += 1;
        } else {
            self.open -= 1;
        }
    }
}
