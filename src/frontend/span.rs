/// Byte range into the source of a chunk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span {
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn slice<'s>(&self, src: &'s [u8]) -> &'s [u8] {
        &src[self.start as usize..self.end as usize]
    }
}
