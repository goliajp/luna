//! The replay of PUC's parser: the arrays and tables it makes, set aside and
//! counted where PUC makes them.

use super::super::*;
use crate::runtime::heap::Heap;
use crate::runtime::mem::Oom;
use crate::runtime::puc::defer::DeferMark;
use crate::runtime::puc::records::parse::LoadTab;
use crate::runtime::puc::records::parse::{FuncArrays, GrowVec, ParserArray, ParserSizes};

/// The replay of PUC's parser: the arrays and tables it makes, set aside and
/// counted where PUC makes them.
pub(crate) trait PucParse {
    /// `luaC_objbarrier` from the main closure `cl` to its new upvalue
    /// `uv`: an upvalue is marked with what it holds at once, never gray.
    fn barrier_new_upval(
        &mut self,
        cl: Gc<crate::runtime::LuaClosure>,
        uv: Gc<crate::runtime::function::Upvalue>,
    );
    /// A new empty parser table (PUC `luaH_new`), counted.
    fn new_load_tab(&mut self) -> Result<Gc<LoadTab>, Oom>;
    /// PUC's `luaC_barrier` as the parser puts `child` in `parent`: while
    /// marking, a black parent's white child is marked; in a sweep, a black
    /// parent not swept yet goes white instead (and nothing waits on a gray
    /// list for a cycle that has not started).
    fn parse_barrier(&mut self, parent: Gc<GcHeader>, child: Gc<GcHeader>);
    /// The bytes of array `a` with room for `n` elements.
    fn parser_bytes(&self, a: ParserArray, n: usize) -> usize;
    /// The function ends (`close_func`): each array trimmed to its count.
    fn parser_close(&mut self, f: &mut FuncArrays);
    /// One instruction on `line` (`luaK_code`): the code array, then the
    /// line info (5.4+ `savelineinfo`, with its absolute entries).
    fn parser_code(&mut self, f: &mut FuncArrays, line: u32);
    /// `v` freed (`luaD_protectedparser` frees the descriptor arrays).
    fn parser_free(&mut self, v: GrowVec, a: ParserArray);
    /// One more element in `v`.
    fn parser_push(&mut self, v: &mut GrowVec, a: ParserArray);
    /// `v` grown to hold `n` (the descriptor arrays, which shrink and grow
    /// again).
    fn parser_reach(&mut self, v: &mut GrowVec, a: ParserArray, n: usize);
    /// What the parser's arrays' bytes depend on now.
    fn parser_sizes(&self) -> ParserSizes;
    /// The last instruction taken back, with its line info (5.4+
    /// `removelastlineinfo`).
    fn parser_uncode(&mut self, f: &mut FuncArrays);
    /// A load calls its reader: what is counted from now on is set aside,
    /// with the objects made meanwhile, until [`Heap::reader_call_ends`]
    /// with the returned mark.
    fn reader_call_begins(&mut self) -> DeferMark;
    /// The reader returned: what it counted since `mark` is taken out of
    /// the count, the objects it made kept aside.
    fn reader_call_ends(&mut self, mark: DeferMark) -> Deferred;
    /// A load with a reader begins: where the counts set aside and the
    /// objects stand now, for [`Heap::reader_load_ends`].
    fn reader_load_begins(&self) -> (isize, usize);
    /// A load that began at `mark` ends: what its reader's calls counted
    /// and no replay counted again is counted now, and the objects they
    /// made go on the list.
    fn reader_load_ends(&mut self, ends: (isize, usize));
    /// The replay reaches where PUC's parser called the reader: what the
    /// call counted is counted now, its objects listed.
    fn replay_reader_call(&mut self, d: Deferred);
    /// The main closure `cl` of a load is built: when a step of this cycle
    /// traversed it as PUC had it (no upvalue yet), its upvalues are marked
    /// too.
    fn settle_closure(&mut self, cl: Gc<crate::runtime::LuaClosure>);
    /// The prototype `p` is built: when a step of this cycle traversed it
    /// as PUC had it then, what luna's own prototype holds besides is
    /// marked too, without counting (PUC's barriers marked the rest as
    /// the parser added it).
    fn settle_parsed(&mut self, p: Gc<crate::runtime::Proto>);
    /// luna's name `bytes`, which PUC makes only where the program first
    /// uses it (`made_later`), and makes again after its sweep freed it:
    /// from 5.3 on, whose sweep goes over strings in the list's order, made
    /// set aside, among the first objects set aside (which no load's
    /// setting aside reaches), so that PUC making it puts it at the list's
    /// head, as PUC's is. See [`Heap::name_aside_again`].
    fn try_intern_name(
        &mut self,
        bytes: &[u8],
        made_later: bool,
    ) -> Result<Gc<crate::runtime::LuaStr>, crate::runtime::mem::Oom>;
}

impl PucParse for Heap {
    #[inline(always)]
    fn barrier_new_upval(
        &mut self,
        cl: Gc<crate::runtime::LuaClosure>,
        uv: Gc<crate::runtime::function::Upvalue>,
    ) {
        Heap::barrier_new_upval(self, cl, uv)
    }

    #[inline(always)]
    fn new_load_tab(&mut self) -> Result<Gc<LoadTab>, Oom> {
        Heap::new_load_tab(self)
    }

    #[inline(always)]
    fn parse_barrier(&mut self, parent: Gc<GcHeader>, child: Gc<GcHeader>) {
        Heap::parse_barrier(self, parent, child)
    }

    #[inline(always)]
    fn parser_bytes(&self, a: ParserArray, n: usize) -> usize {
        Heap::parser_bytes(self, a, n)
    }

    #[inline(always)]
    fn parser_close(&mut self, f: &mut FuncArrays) {
        Heap::parser_close(self, f)
    }

    #[inline(always)]
    fn parser_code(&mut self, f: &mut FuncArrays, line: u32) {
        Heap::parser_code(self, f, line)
    }

    #[inline(always)]
    fn parser_free(&mut self, v: GrowVec, a: ParserArray) {
        Heap::parser_free(self, v, a)
    }

    #[inline(always)]
    fn parser_push(&mut self, v: &mut GrowVec, a: ParserArray) {
        Heap::parser_push(self, v, a)
    }

    #[inline(always)]
    fn parser_reach(&mut self, v: &mut GrowVec, a: ParserArray, n: usize) {
        Heap::parser_reach(self, v, a, n)
    }

    #[inline(always)]
    fn parser_sizes(&self) -> ParserSizes {
        Heap::parser_sizes(self)
    }

    #[inline(always)]
    fn parser_uncode(&mut self, f: &mut FuncArrays) {
        Heap::parser_uncode(self, f)
    }

    #[inline(always)]
    fn reader_call_begins(&mut self) -> DeferMark {
        Heap::reader_call_begins(self)
    }

    #[inline(always)]
    fn reader_call_ends(&mut self, mark: DeferMark) -> Deferred {
        Heap::reader_call_ends(self, mark)
    }

    #[inline(always)]
    fn reader_load_begins(&self) -> (isize, usize) {
        Heap::reader_load_begins(self)
    }

    #[inline(always)]
    fn reader_load_ends(&mut self, ends: (isize, usize)) {
        Heap::reader_load_ends(self, ends)
    }

    #[inline(always)]
    fn replay_reader_call(&mut self, d: Deferred) {
        Heap::replay_reader_call(self, d)
    }

    #[inline(always)]
    fn settle_closure(&mut self, cl: Gc<crate::runtime::LuaClosure>) {
        Heap::settle_closure(self, cl)
    }

    #[inline(always)]
    fn settle_parsed(&mut self, p: Gc<crate::runtime::Proto>) {
        Heap::settle_parsed(self, p)
    }

    #[inline(always)]
    fn try_intern_name(
        &mut self,
        bytes: &[u8],
        made_later: bool,
    ) -> Result<Gc<crate::runtime::LuaStr>, crate::runtime::mem::Oom> {
        Heap::try_intern_name(self, bytes, made_later)
    }
}
