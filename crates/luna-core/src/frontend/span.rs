//! Source byte spans used by the lexer / parser to point back into the
//! original chunk text for error reporting and `Token::describe`.

/// Byte range into the source of a chunk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: u32,
    /// End byte offset (exclusive).
    pub end: u32,
}

impl Span {
    /// Build a span over `[start, end)`.
    pub fn new(start: usize, end: usize) -> Span {
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    /// Borrow the source bytes the span covers.
    pub fn slice<'s>(&self, src: &'s [u8]) -> &'s [u8] {
        &src[self.start as usize..self.end as usize]
    }
}
