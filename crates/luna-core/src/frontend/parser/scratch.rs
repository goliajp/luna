//! The parser's vectors, kept by a VM between loads.

use super::*;
use crate::runtime::mem::{LVec, MemOwner, MemRef};

/// Lists being collected while their items are parsed. Lists nest (a
/// call's arguments hold calls), so each kind of item gathers on a stack
/// and a finished list is moved to the chunk in one piece ([`finish`]).
pub(crate) struct ListStacks {
    pub(super) exprs: LVec<ExprId>,
    pub(super) stats: LVec<StatId>,
    pub(super) names: LVec<Name>,
    pub(super) attribs: LVec<AttribName>,
    pub(super) fields: LVec<TableField>,
    pub(super) arms: LVec<IfArm>,
    /// the parser's per-function stacks and goto checker
    pub(super) func_local_count: LVec<(u32, u32, u32)>,
    pub(super) funcs: LVec<FnFlow>,
    pub(super) gotos: Option<GotoCheck>,
}

impl ListStacks {
    pub(super) fn new(mem: MemRef) -> ListStacks {
        ListStacks {
            exprs: LVec::new(mem),
            stats: LVec::new(mem),
            names: LVec::new(mem),
            attribs: LVec::new(mem),
            fields: LVec::new(mem),
            arms: LVec::new(mem),
            func_local_count: LVec::new(mem),
            funcs: LVec::new(mem),
            gotos: None,
        }
    }
}

/// The items gathered on `stack` since `mark`, moved to `chunk` as a list.
pub(super) fn finish<T: ListItem>(chunk: &mut Chunk, stack: &mut LVec<T>, mark: usize) -> List<T> {
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
pub(crate) struct ParseScratch {
    /// a tree with every vector emptied
    pub(super) chunk: Chunk,
    pub(super) end_lines: LVec<u32>,
    pub(super) lex_buf: LVec<u8>,
    pub(super) stacks: ListStacks,
}

impl ParseScratch {
    /// Empty vectors on `mem`, nothing allocated.
    pub(crate) fn new(mem: MemOwner) -> ParseScratch {
        let m = mem.mem();
        ParseScratch {
            chunk: Chunk::new(mem),
            end_lines: LVec::new(m),
            lex_buf: LVec::new(m),
            stacks: ListStacks::new(m),
        }
    }

    /// The most expression slots kept between loads: a huge chunk's vectors
    /// are given back rather than held on to.
    const MAX_EXPRS: usize = 1 << 14;

    /// The handle the vectors allocate through.
    pub(crate) fn mem(&self) -> MemRef {
        self.chunk.mem_owner().mem()
    }

    /// The vectors, leaving empty ones on the same context.
    pub(crate) fn take(&mut self) -> ParseScratch {
        let fresh = ParseScratch::new(self.chunk.mem_owner().clone());
        std::mem::replace(self, fresh)
    }

    /// Empty the vectors of a finished parse for the next one.
    pub(crate) fn recycle(p: Parsed) -> ParseScratch {
        let mut s = ParseScratch {
            chunk: p.chunk,
            end_lines: p.end_lines,
            lex_buf: p.lex_buf,
            stacks: p.stacks,
        };
        s.clear();
        s
    }

    /// Empty the tree's vectors (left filled by the last parse, or half
    /// filled by one that failed), giving back a huge chunk's.
    pub(super) fn clear(&mut self) {
        let chunk = &mut self.chunk;
        if chunk.exprs.capacity() > Self::MAX_EXPRS {
            *self = ParseScratch::new(self.chunk.mem_owner().clone());
            return;
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
        chunk.fold_marks.clear();
        self.end_lines.clear();
        // a parse that failed leaves the lists it was gathering
        let st = &mut self.stacks;
        st.exprs.clear();
        st.stats.clear();
        st.names.clear();
        st.attribs.clear();
        st.fields.clear();
        st.arms.clear();
    }

    /// The tree and statement end lines [`parse_in_place`] left here.
    ///
    /// [`parse_in_place`]: super::parse_in_place
    pub(crate) fn parsed(&self) -> (&Chunk, &[u32]) {
        (&self.chunk, &self.end_lines)
    }

    /// The parse left here, as the owner of its vectors.
    pub(super) fn into_parsed(self) -> Parsed {
        Parsed {
            chunk: self.chunk,
            end_lines: self.end_lines,
            lex_buf: self.lex_buf,
            stacks: self.stacks,
        }
    }
}
