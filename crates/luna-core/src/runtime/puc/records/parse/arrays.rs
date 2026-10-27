//! The arrays PUC's parser grows as it compiles a function (code, line
//! info, constants, prototypes, local and upvalue descriptions) and its
//! dynamic data (`Dyndata`: active variables, labels, gotos): doubled when
//! full (`luaM_growvector`), trimmed when the function ends
//! (`luaM_shrinkvector`), each element as many bytes as PUC's.

use super::*;
use crate::runtime::puc::size::{PucSizes, dyd_sizes};

/// One of the parser's arrays.
#[derive(Clone, Copy)]
pub(crate) enum ParserArray {
    Code,
    Protos,
    Consts,
    LineInfo,
    AbsLines,
    LocVars,
    Upvals,
    ActVar,
    Labels,
    Gotos,
}

/// An array as PUC grows it: `n` elements, room for `cap`.
#[derive(Clone, Copy, Default)]
pub(crate) struct GrowVec {
    pub(crate) n: usize,
    pub(crate) cap: usize,
}

/// The arrays of a function being compiled, and where its line info
/// stands (5.4+ `savelineinfo`).
#[derive(Clone, Copy, Default)]
pub(crate) struct FuncArrays {
    pub(crate) code: GrowVec,
    pub(crate) line: GrowVec,
    pub(crate) abs: GrowVec,
    pub(crate) k: GrowVec,
    pub(crate) p: GrowVec,
    pub(crate) loc: GrowVec,
    pub(crate) up: GrowVec,
    /// the line of the last instruction
    pub(crate) prev_line: u32,
    iwthabs: u32,
    /// the last instruction's line entry: absolute, and the line before it
    /// (to take it back, PUC `removelastlineinfo`)
    last_abs: bool,
    last_prev: u32,
}

impl FuncArrays {
    /// A function defined on line `line_defined`.
    pub(crate) fn new(line_defined: u32) -> FuncArrays {
        FuncArrays {
            prev_line: line_defined,
            ..FuncArrays::default()
        }
    }
}

/// What the bytes of the parser's arrays depend on (the dialect, the
/// sizes), for a count made away from the heap.
#[derive(Clone, Copy)]
pub(crate) struct ParserSizes {
    d: Dialect,
    s: PucSizes,
}

impl ParserSizes {
    /// The bytes of one element of array `a`.
    #[inline(always)]
    fn elem(&self, a: ParserArray) -> usize {
        let (s, d) = (&self.s, self.d);
        match a {
            ParserArray::Code => 4,
            ParserArray::Protos => s.ptr,
            ParserArray::Consts => s.tvalue,
            ParserArray::LineInfo if matches!(d, Dialect::L54 | Dialect::L55) => 1,
            ParserArray::LineInfo => 4,
            ParserArray::AbsLines => s.abslineinfo,
            ParserArray::LocVars => s.locvar,
            ParserArray::Upvals => s.upvaldesc,
            ParserArray::ActVar => dyd_sizes(d).0,
            ParserArray::Labels | ParserArray::Gotos => dyd_sizes(d).1,
        }
    }

    /// The most elements PUC lets array `a` hold (the `limit` its
    /// `luaM_growvector` passes; from 5.4 also what `size_t` can count).
    fn limit(&self, a: ParserArray) -> usize {
        const MAX_INT: usize = i32::MAX as usize;
        const SHRT_MAX: usize = i16::MAX as usize;
        let d = self.d;
        let modern = matches!(d, Dialect::L54 | Dialect::L55);
        let n = match a {
            ParserArray::Code | ParserArray::LineInfo | ParserArray::AbsLines => MAX_INT,
            // MAXARG_Bx (5.1's constants too) and MAXARG_Ax
            ParserArray::Consts if d == Dialect::L51 => (1 << 18) - 1,
            ParserArray::Consts if modern => (1 << 25) - 1,
            ParserArray::Consts => (1 << 26) - 1,
            ParserArray::Protos if modern => (1 << 17) - 1,
            ParserArray::Protos => (1 << 18) - 1,
            ParserArray::LocVars | ParserArray::Labels | ParserArray::Gotos => SHRT_MAX,
            // MAXUPVAL; 5.1 checks its own cap of 60 apart
            ParserArray::Upvals if d == Dialect::L51 => MAX_INT,
            ParserArray::Upvals => 255,
            ParserArray::ActVar if modern => SHRT_MAX,
            ParserArray::ActVar => MAX_INT,
        };
        if modern {
            // `luaM_limitN`, on the counted platform's `size_t`
            let max_size = if self.s.ptr == 4 {
                u32::MAX as usize
            } else {
                usize::MAX
            };
            n.min(max_size / self.elem(a))
        } else {
            n
        }
    }
}

impl FuncArrays {
    /// The bytes of each of the function's prototype arrays as grown, in
    /// the order `Proto::puc_arrays` keeps them.
    pub(crate) fn caps(&self, ps: &ParserSizes) -> [u32; 7] {
        [
            (self.code, ParserArray::Code),
            (self.p, ParserArray::Protos),
            (self.k, ParserArray::Consts),
            (self.line, ParserArray::LineInfo),
            (self.abs, ParserArray::AbsLines),
            (self.loc, ParserArray::LocVars),
            (self.up, ParserArray::Upvals),
        ]
        .map(|(v, a)| (v.cap * ps.elem(a)) as u32)
    }
}

impl Heap {
    /// What the parser's arrays' bytes depend on now.
    #[inline]
    pub(in crate::runtime) fn parser_sizes(&self) -> ParserSizes {
        ParserSizes {
            d: self.puc.dialect,
            s: self.puc.sizes,
        }
    }

    /// The bytes of one element of array `a`.
    #[inline(always)]
    fn parser_elem(&self, a: ParserArray) -> usize {
        self.parser_sizes().elem(a)
    }

    /// The bytes of array `a` with room for `n` elements.
    pub(in crate::runtime) fn parser_bytes(&self, a: ParserArray, n: usize) -> usize {
        n * self.parser_elem(a)
    }

    /// One more element in `v`.
    #[inline(always)]
    pub(in crate::runtime) fn parser_push(&mut self, v: &mut GrowVec, a: ParserArray) {
        if v.n + 1 > v.cap {
            self.parser_grow(v, a);
        }
        v.n += 1;
    }

    /// `v` grown to hold `n` (the descriptor arrays, which shrink and grow
    /// again).
    pub(in crate::runtime) fn parser_reach(&mut self, v: &mut GrowVec, a: ParserArray, n: usize) {
        while v.cap < n {
            self.parser_grow(v, a);
        }
        v.n = n;
    }

    /// `v` doubled (at least 4), or grown to its limit once doubling would
    /// pass half of it (`luaM_growaux_`).
    fn parser_grow(&mut self, v: &mut GrowVec, a: ParserArray) {
        let ps = self.parser_sizes();
        let limit = ps.limit(a);
        let new = if v.cap >= limit / 2 {
            limit
        } else {
            (v.cap * 2).max(4)
        };
        let elem = ps.elem(a);
        self.acct_block(v.cap * elem, new * elem);
        v.cap = new;
    }

    /// `v` freed (`luaD_protectedparser` frees the descriptor arrays).
    pub(in crate::runtime) fn parser_free(&mut self, v: GrowVec, a: ParserArray) {
        let b = self.parser_bytes(a, v.cap);
        self.acct_block(b, 0);
    }

    /// `v` trimmed to its elements.
    fn parser_trim(&mut self, v: &mut GrowVec, a: ParserArray) {
        let elem = self.parser_elem(a);
        self.acct_block(v.cap * elem, v.n * elem);
        v.cap = v.n;
    }

    /// One instruction on `line` (`luaK_code`): the code array, then the
    /// line info (5.4+ `savelineinfo`, with its absolute entries).
    #[inline(always)]
    pub(in crate::runtime) fn parser_code(&mut self, f: &mut FuncArrays, line: u32) {
        self.parser_push(&mut f.code, ParserArray::Code);
        if matches!(self.puc.dialect, Dialect::L54 | Dialect::L55) {
            let dif = i64::from(line) - i64::from(f.prev_line);
            let wthabs = f.iwthabs;
            f.iwthabs += 1;
            f.last_prev = f.prev_line;
            f.last_abs = dif.abs() >= 0x80 || wthabs >= 128;
            if f.last_abs {
                self.parser_push(&mut f.abs, ParserArray::AbsLines);
                f.iwthabs = 1;
            }
            f.prev_line = line;
        }
        self.parser_push(&mut f.line, ParserArray::LineInfo);
    }

    /// The last instruction taken back, with its line info (5.4+
    /// `removelastlineinfo`).
    pub(in crate::runtime) fn parser_uncode(&mut self, f: &mut FuncArrays) {
        f.code.n -= 1;
        f.line.n -= 1;
        if f.last_abs {
            f.abs.n -= 1;
            f.iwthabs = 129;
        } else {
            f.prev_line = f.last_prev;
            f.iwthabs = f.iwthabs.saturating_sub(1);
        }
    }

    /// The function ends (`close_func`): each array trimmed to its count.
    pub(in crate::runtime) fn parser_close(&mut self, f: &mut FuncArrays) {
        self.parser_trim(&mut f.code, ParserArray::Code);
        self.parser_trim(&mut f.line, ParserArray::LineInfo);
        self.parser_trim(&mut f.abs, ParserArray::AbsLines);
        self.parser_trim(&mut f.k, ParserArray::Consts);
        self.parser_trim(&mut f.p, ParserArray::Protos);
        self.parser_trim(&mut f.loc, ParserArray::LocVars);
        self.parser_trim(&mut f.up, ParserArray::Upvals);
    }
}
