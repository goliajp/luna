//! The replay of PUC's parser: the arrays and tables it makes, set aside and
//! counted where PUC makes them.

use super::super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::mem::{LVec, Oom};
use crate::runtime::puc::records::parse::*;

impl Heap {
    /// A new empty parser table (PUC `luaH_new`), counted.
    pub(crate) fn new_load_tab(&mut self) -> Result<Gc<LoadTab>, Oom> {
        let mut hdr = GcHeader::new(ObjTag::Ghost);
        (hdr.sub, hdr.cls) = (LOADTAB, 0);
        hdr.set_flags(0);
        let refs = LVec::new(self.mem());
        let t = self.try_adopt(LoadTab {
            hdr,
            asize: 0,
            nodes: 0,
            counted: self.counting(),
            refs,
        })?;
        self.acct_obj(PucKind::Table, self.puc.sizes.table);
        Ok(t)
    }

    /// `luaC_objbarrier` from the main closure `cl` to its new upvalue
    /// `uv`: an upvalue is marked with what it holds at once, never gray.
    pub(crate) fn barrier_new_upval(
        &mut self,
        cl: Gc<crate::runtime::LuaClosure>,
        uv: Gc<crate::runtime::function::Upvalue>,
    ) {
        let (c, u) = (cl.header(), uv.header());
        // SAFETY: both are live objects of this heap; only flag bytes are
        // read and written
        unsafe {
            if !is_black((*c).flags()) || !is_white((*u).flags()) {
                return;
            }
            // in a sweep the closure goes white instead (`luaC_barrier`)
            if self.phase() != GcPhase::Propagate {
                (*c).paint(self.white());
                return;
            }
            if self.puc.pace.count_marked && self.phase() == GcPhase::Propagate {
                self.count_marked(u);
            }
            (*u).paint(BLACK);
        }
        if let crate::runtime::function::UpvalState::Closed(v) = uv.state {
            self.barrier_forward(uv, v);
        }
    }

    /// What the parser's arrays' bytes depend on now.
    #[inline]
    pub(crate) fn parser_sizes(&self) -> ParserSizes {
        ParserSizes {
            d: self.puc.dialect,
            s: self.puc.sizes,
        }
    }

    /// The bytes of array `a` with room for `n` elements.
    pub(crate) fn parser_bytes(&self, a: ParserArray, n: usize) -> usize {
        n * self.parser_elem(a)
    }

    /// One more element in `v`.
    #[inline(always)]
    pub(crate) fn parser_push(&mut self, v: &mut GrowVec, a: ParserArray) {
        if v.n + 1 > v.cap {
            self.parser_grow(v, a);
        }
        v.n += 1;
    }

    /// `v` grown to hold `n` (the descriptor arrays, which shrink and grow
    /// again).
    pub(crate) fn parser_reach(&mut self, v: &mut GrowVec, a: ParserArray, n: usize) {
        while v.cap < n {
            self.parser_grow(v, a);
        }
        v.n = n;
    }

    /// `v` freed (`luaD_protectedparser` frees the descriptor arrays).
    pub(crate) fn parser_free(&mut self, v: GrowVec, a: ParserArray) {
        let b = self.parser_bytes(a, v.cap);
        self.acct_block(b, 0);
    }

    /// The function ends (`close_func`): each array trimmed to its count.
    pub(crate) fn parser_close(&mut self, f: &mut FuncArrays) {
        self.parser_trim(&mut f.code, ParserArray::Code);
        self.parser_trim(&mut f.line, ParserArray::LineInfo);
        self.parser_trim(&mut f.abs, ParserArray::AbsLines);
        self.parser_trim(&mut f.k, ParserArray::Consts);
        self.parser_trim(&mut f.p, ParserArray::Protos);
        self.parser_trim(&mut f.loc, ParserArray::LocVars);
        self.parser_trim(&mut f.up, ParserArray::Upvals);
    }

    /// One instruction coded on `line` (`luaK_code`): the code array, then
    /// the line info (5.4+ `savelineinfo`, with its absolute entries).
    #[inline(always)]
    pub(crate) fn parser_code(&mut self, f: &mut FuncArrays, line: u32) {
        self.parser_push(&mut f.code, ParserArray::Code);
        if matches!(self.puc.dialect, Dialect::L54 | Dialect::L55) {
            self.save_line(f, line);
        }
        self.parser_push(&mut f.line, ParserArray::LineInfo);
    }

    /// The last instruction moved to `line` (5.4+ `luaK_fixline`: its line
    /// taken back and saved again).
    pub(crate) fn parser_fix(&mut self, f: &mut FuncArrays, line: u32) {
        if matches!(self.puc.dialect, Dialect::L54 | Dialect::L55) {
            f.unsave_line();
            self.save_line(f, line);
        }
    }

    /// The last instruction taken back, with its line info (5.4+
    /// `removelastlineinfo`).
    pub(crate) fn parser_uncode(&mut self, f: &mut FuncArrays) {
        f.code.n -= 1;
        f.line.n -= 1;
        f.unsave_line();
    }

    /// PUC's `luaC_barrier` as the parser puts `child` in `parent`: while
    /// marking, a black parent's white child is marked; in a sweep, a black
    /// parent not swept yet goes white instead (and nothing waits on a gray
    /// list for a cycle that has not started).
    pub(crate) fn parse_barrier(&mut self, parent: Gc<GcHeader>, child: Gc<GcHeader>) {
        let (parent, child) = (parent.as_ptr(), child.as_ptr());
        // SAFETY: both are handles, so their objects are live; only flag
        // bytes are read and written
        unsafe {
            if self.phase() == GcPhase::Propagate {
                self.barrier_forward_obj(parent, child);
            } else if is_black((*parent).flags()) {
                (*parent).paint(self.white());
            }
        }
    }

    /// The prototype `p` is built: when a step of this cycle traversed it
    /// as PUC had it then, what luna's own prototype holds besides is
    /// marked too, without counting (PUC's barriers marked the rest as
    /// the parser added it).
    pub(crate) fn settle_parsed(&mut self, p: Gc<crate::runtime::Proto>) {
        let h = p.as_ptr() as *mut GcHeader;
        // SAFETY: a live prototype; only flag bytes are read and written
        unsafe {
            if self.phase() != GcPhase::Propagate || !is_black((*h).flags()) {
                return;
            }
            let strs = p.consts.iter().filter_map(|k| match k {
                Value::Str(s) => Some(*s),
                _ => None,
            });
            let names = p.puc_names.borrow();
            for s in strs.chain(names.iter().copied()).chain([p.source]) {
                let sh = s.as_ptr() as *mut GcHeader;
                let f = (*sh).flags();
                if is_white(f) {
                    (*sh).paint(BLACK);
                }
            }
            for &c in p.protos.iter() {
                self.barrier_forward_obj(h, c.as_ptr() as *mut GcHeader);
            }
        }
    }

    /// The main closure `cl` of a load is built: when a step of this cycle
    /// traversed it as PUC had it (no upvalue yet), its upvalues are marked
    /// too.
    pub(crate) fn settle_closure(&mut self, cl: Gc<crate::runtime::LuaClosure>) {
        for &uv in cl.upvals().iter() {
            self.barrier_new_upval(cl, uv);
        }
    }

    /// luna's name `bytes`, which PUC makes only where the program first
    /// uses it (`made_later`), and makes again after its sweep freed it:
    /// from 5.3 on, whose sweep goes over strings in the list's order, made
    /// set aside, among the first objects set aside (which no load's
    /// setting aside reaches), so that PUC making it puts it at the list's
    /// head, as PUC's is. See [`Heap::name_aside_again`].
    pub(crate) fn try_intern_name(
        &mut self,
        bytes: &[u8],
        made_later: bool,
    ) -> Result<Gc<crate::runtime::LuaStr>, crate::runtime::mem::Oom> {
        if !made_later || self.puc.rules.sweeps_string_table {
            return self.try_intern(bytes);
        }
        let a = self.set_loads_aside(true);
        let s = self.try_intern(bytes);
        self.names_aside_back(a);
        s
    }

    /// The object of `g` is one the parser is building (`on`), or no more:
    /// PUC's parser has not finished it, so a cycle marks it as PUC does.
    pub(crate) fn set_parsing<T: GcObject>(&mut self, g: Gc<T>, on: bool) {
        let h = g.header();
        // SAFETY: a handle's object is allocated; only its class byte is written
        unsafe {
            if on {
                (*h).cls |= PARSING;
            } else {
                (*h).cls &= !PARSING;
            }
        }
    }
}
