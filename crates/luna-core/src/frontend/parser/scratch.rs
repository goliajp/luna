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
    /// goto and read-only checks pass whatever they meet (see
    /// [`ParseScratch::lenient`])
    pub(super) lenient: bool,
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
            lenient: false,
        }
    }
}

/// What `...` and `break` are checked against in an open function.
pub(crate) struct FnFlow {
    pub(super) vararg: bool,
    /// Loops enclosing the current position inside this function.
    pub(super) loops: u32,
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

/// A parsed chunk with what the public [`Chunk`] has no place for.
pub(crate) struct Parsed {
    pub(crate) chunk: Chunk,
    /// the line of each statement's last token, by `StatId`: PUC attributes
    /// the code it emits once it has read that token (a loop's `end`) to
    /// its line
    pub(crate) end_lines: LVec<u32>,
    /// the lexer's token buffer, kept for the next load
    pub(crate) lex_buf: LVec<u8>,
    /// the parser's list stacks, kept for the next load
    pub(crate) stacks: ListStacks,
    /// the source of a parse that read it piece by piece, where a compile
    /// error is placed (a whole source is the caller's)
    pub(crate) src: Option<LVec<u8>>,
    /// what a recording parse recorded (see [`ParseScratch::recorded`])
    recording: Option<Box<marks::Recording>>,
    recorded: bool,
}

impl Parsed {
    /// What the parse recorded, when it was asked to.
    pub(crate) fn recorded(&self) -> Option<(&marks::Marks, &marks::scan::Scanned)> {
        let r = self.recording.as_deref().filter(|_| self.recorded)?;
        Some((&r.marks, &r.scanned))
    }
}

/// Where a parse failed: at token `at` (the first is 0), or reading the
/// token after it (`scanning`, from byte `scan.0` of the source to
/// `scan.1`), or reading the first token (`first`); and what the error
/// quotes as its near-token.
#[derive(Clone, Copy)]
pub(crate) struct Failed {
    pub(crate) at: u32,
    pub(crate) scanning: bool,
    pub(crate) first: bool,
    pub(crate) scan: (u32, u32),
    pub(crate) near: crate::frontend::error::NearKind,
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
    /// the source of a streamed parse (see [`Parsed::src`])
    pub(crate) streamed: Option<LVec<u8>>,
    /// where the last parse failed, if it did
    pub(crate) failed: Option<Failed>,
    /// the next parse records its marks and the strings PUC's scanner
    /// makes, for a load that is counted
    pub(crate) record: bool,
    /// and where each token ends, which a streamed parse always records
    pub(crate) record_ends: bool,
    /// the last recording, kept for its vectors; valid while `recorded`
    pub(super) recording: Option<Box<marks::Recording>>,
    pub(super) recorded: bool,
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
            streamed: None,
            failed: None,
            record: false,
            record_ends: false,
            recording: None,
            recorded: false,
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
            streamed: None,
            failed: None,
            record: false,
            record_ends: false,
            recording: p.recording,
            recorded: false,
        };
        s.clear();
        s
    }

    /// Give back the vectors of a huge chunk's parse, rather than hold on
    /// to them: whether they were.
    pub(crate) fn release_huge(&mut self) -> bool {
        if self.chunk.exprs.capacity() <= Self::MAX_EXPRS {
            return false;
        }
        let record = self.record;
        *self = ParseScratch::new(self.chunk.mem_owner().clone());
        self.record = record;
        true
    }

    /// Empty the tree's vectors (left filled by the last parse, or half
    /// filled by one that failed), giving back a huge chunk's.
    pub(super) fn clear(&mut self) {
        self.recorded = false;
        if self.release_huge() {
            return;
        }
        let chunk = &mut self.chunk;
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
        chunk.dot_keys.clear();
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

    /// The first token failed to scan: the parse failed with nothing read.
    pub(super) fn first_failed(&mut self, mut lex: TokenSource, e: SyntaxError) -> SyntaxError {
        if let Some(o) = lex.out_of_memory() {
            return o.into();
        }
        let (near, start, pos) = lex.scan_state();
        self.failed = Some(Failed {
            at: 0,
            scanning: true,
            first: true,
            scan: (start as u32, pos as u32),
            near,
        });
        let ends = matches!(lex, TokenSource::Stream(_));
        self.recording = Some(marks::Recording::cleared(self.recording.take(), ends));
        self.recorded = false;
        self.streamed = lex.take_streamed();
        self.chunk.names = lex.take_names();
        e
    }

    /// The tree and statement end lines [`parse_in_place`] left here.
    ///
    /// [`parse_in_place`]: super::parse_in_place
    pub(crate) fn parsed(&self) -> (&Chunk, &[u32]) {
        (&self.chunk, &self.end_lines)
    }

    /// The next parses take as valid a goto that finds no label, a label
    /// defined twice, an assignment to a read-only local, an unknown
    /// attribute, nesting a little past the limit and more locals than the
    /// limit: a failed load is replayed from a completion of its source
    /// that has the construct it failed on.
    pub(crate) fn lenient(&mut self, on: bool) {
        self.stacks.lenient = on;
    }

    /// What the last parse recorded, when it was asked to (see
    /// [`ParseScratch::record`]) and succeeded.
    pub(crate) fn recorded(&self) -> Option<(&marks::Marks, &marks::scan::Scanned)> {
        let r = self.recording.as_deref().filter(|_| self.recorded)?;
        Some((&r.marks, &r.scanned))
    }

    /// What a recording parse that failed scanned before it failed, and
    /// the names of the tokens.
    pub(crate) fn failed_scan(
        &self,
    ) -> Option<(&marks::scan::Scanned, &crate::frontend::names::Names)> {
        self.failed?;
        let r = self.recording.as_deref().filter(|_| self.record)?;
        Some((&r.scanned, &self.chunk.names))
    }

    /// How many statements and expressions a recording parse that failed
    /// had read to their end: the nodes before those numbers (see
    /// [`Marks`]) are the same in a completion of its source.
    pub(crate) fn failed_closed(&self) -> (u32, u32) {
        let m = self.recording.as_deref().map(|r| &r.marks);
        m.map_or((0, 0), |m| {
            (m.stat_follow.len() as u32, m.expr_follow.len() as u32)
        })
    }

    /// The parse left here, as the owner of its vectors.
    pub(super) fn into_parsed(self) -> Parsed {
        Parsed {
            chunk: self.chunk,
            end_lines: self.end_lines,
            lex_buf: self.lex_buf,
            stacks: self.stacks,
            src: self.streamed,
            recording: self.recording,
            recorded: self.recorded,
        }
    }
}
