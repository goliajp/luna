//! The parser's vectors, kept by a VM between loads.

use super::*;

/// The tree's vectors, empty, handed to [`parse_from_source`].
pub(super) type Arenas = (Vec<Expr>, Vec<Stat>, Vec<u32>, Vec<u32>);

/// The vectors a parse builds its tree in, kept by a VM from one load to
/// the next: a load then starts with the room the last one needed instead
/// of growing every vector from empty.
#[derive(Default)]
pub(crate) struct ParseScratch {
    pub(super) exprs: Vec<Expr>,
    pub(super) stats: Vec<Stat>,
    pub(super) stat_lines: Vec<u32>,
    pub(super) end_lines: Vec<u32>,
    pub(super) names: Names,
    pub(super) lex_buf: Vec<u8>,
}

impl ParseScratch {
    /// The most expression slots kept between loads: a huge chunk's vectors
    /// are given back rather than held on to.
    const MAX_EXPRS: usize = 1 << 14;

    /// Empty the vectors of a finished parse for the next one.
    pub(crate) fn recycle(p: Parsed) -> ParseScratch {
        let Parsed {
            chunk,
            names,
            end_lines,
            lex_buf,
        } = p;
        if chunk.exprs.capacity() > Self::MAX_EXPRS {
            return ParseScratch::default();
        }
        let Chunk {
            mut exprs,
            mut stats,
            mut stat_lines,
            ..
        } = chunk;
        let mut end_lines = end_lines;
        exprs.clear();
        stats.clear();
        stat_lines.clear();
        end_lines.clear();
        ParseScratch {
            exprs,
            stats,
            stat_lines,
            end_lines,
            names,
            lex_buf,
        }
    }
}
