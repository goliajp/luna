//! The parser's vectors, kept by a VM between loads.

use super::*;

/// Lists being collected while their items are parsed. Lists nest (a
/// call's arguments hold calls), so each kind of item gathers on a stack
/// and a finished list is moved to the chunk in one piece ([`finish`]).
#[derive(Default)]
pub(crate) struct ListStacks {
    pub(super) exprs: Vec<ExprId>,
    pub(super) stats: Vec<StatId>,
    pub(super) names: Vec<Name>,
    pub(super) attribs: Vec<AttribName>,
    pub(super) fields: Vec<TableField>,
    pub(super) arms: Vec<IfArm>,
}

/// The items gathered on `stack` since `mark`, moved to `chunk` as a list.
pub(super) fn finish<T: ListItem>(chunk: &mut Chunk, stack: &mut Vec<T>, mark: usize) -> List<T> {
    if stack.len() == mark {
        return List::EMPTY;
    }
    let l = chunk.push_list(&stack[mark..]);
    stack.truncate(mark);
    l
}

/// The vectors a parse builds its tree in, kept by a VM from one load to
/// the next: a load then starts with the room the last one needed instead
/// of growing every vector from empty.
#[derive(Default)]
pub(crate) struct ParseScratch {
    /// a tree with every vector emptied
    pub(super) chunk: Chunk,
    pub(super) end_lines: Vec<u32>,
    pub(super) lex_buf: Vec<u8>,
    pub(super) stacks: ListStacks,
}

impl ParseScratch {
    /// The most expression slots kept between loads: a huge chunk's vectors
    /// are given back rather than held on to.
    const MAX_EXPRS: usize = 1 << 14;

    /// Empty the vectors of a finished parse for the next one.
    pub(crate) fn recycle(p: Parsed) -> ParseScratch {
        let Parsed {
            mut chunk,
            mut end_lines,
            lex_buf,
            stacks,
        } = p;
        if chunk.exprs.capacity() > Self::MAX_EXPRS {
            return ParseScratch::default();
        }
        chunk.exprs.clear();
        chunk.stats.clear();
        chunk.stat_lines.clear();
        chunk.expr_lists.clear();
        chunk.stat_lists.clear();
        chunk.name_lists.clear();
        chunk.attrib_name_lists.clear();
        chunk.field_lists.clear();
        chunk.arm_lists.clear();
        end_lines.clear();
        ParseScratch {
            chunk,
            end_lines,
            lex_buf,
            stacks,
        }
    }
}
