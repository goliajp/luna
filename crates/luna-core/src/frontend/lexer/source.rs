//! Where a lexer's bytes come from: a whole source in memory, or a reader
//! asked for the next piece as the scan moves past the end of what it has
//! (PUC's `ZIO`), so a syntax error stops the reading where PUC stops it.

use crate::runtime::mem::{LVec, Oom};

/// The bytes a [`super::Lexer`] scans.
#[doc(hidden)]
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
}

/// A source held whole in memory.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct Whole<'s>(pub(crate) &'s [u8]);

impl Source for Whole<'_> {
    #[inline(always)]
    fn bytes(&self) -> &[u8] {
        self.0
    }
}

/// A reader of the next piece: it appends one piece to the buffer and
/// returns true, or returns false at the end of the input (PUC's
/// `lua_Reader` returning no bytes); an error when the buffer cannot grow.
pub(crate) type Feed<'f> = dyn FnMut(&mut LVec<u8>) -> Result<bool, Oom> + 'f;

/// A source read piece by piece. Once the reader has signalled the end it
/// is not called again: PUC's scanner stops at the end of input.
pub(crate) struct Stream<'f> {
    buf: LVec<u8>,
    feed: &'f mut Feed<'f>,
    ended: bool,
    /// reading stopped because `buf` could not grow
    oom: Option<Oom>,
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
        }
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
        match (self.feed)(&mut self.buf) {
            Ok(more) if more && self.buf.len() > before => {}
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
}
