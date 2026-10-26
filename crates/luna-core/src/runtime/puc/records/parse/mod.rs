//! What a load's parser holds while a collection step runs in the middle
//! of the load (PUC checks for a collection as it parses): its tables,
//! records of PUC's on the object list ([`LoadTab`]), and for each load in
//! progress a frame naming the objects on the loading thread's stack and
//! what each prototype being built holds so far ([`ParseFrame`]).

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::mem::{LVec, Oom};

mod arrays;
mod mark;
mod table;
mod unborn;
#[cfg(test)]
mod unborn_tests;
pub(crate) use arrays::{FuncArrays, GrowVec, ParserArray, ParserSizes};
pub(crate) use table::{PKey, PTab};
pub(in crate::runtime) use unborn::LoadsAside;

/// In a record's `sub` byte: a parser table.
const LOADTAB: u8 = 0x20;
/// In a prototype's or a closure's `cls`: the parser is building it.
pub(crate) const PARSING: u8 = 0x20;
/// In a string's `cls`: only a load in progress holds it, which PUC has
/// not made it for yet. PUC frees it; luna keeps it, uncounted.
pub(in crate::runtime) const KEPT: u8 = 0x20;

/// [`KEPT`] for a string, nothing for another object (whose bit means
/// something else).
#[inline(always)]
pub(in crate::runtime) fn kept_bit(tag: ObjTag) -> u8 {
    if tag == ObjTag::Str { KEPT } else { 0 }
}

/// One of PUC's parser tables, which luna has no table for: its parts'
/// sizes and the objects among its keys and values.
#[repr(C)]
pub(crate) struct LoadTab {
    pub(crate) hdr: GcHeader,
    asize: u32,
    nodes: u32,
    /// made while counting, so counted as it grows and when it is freed
    counted: bool,
    pub(super) refs: LVec<*mut GcHeader>,
}

/// A prototype being built, as PUC's holds it so far: the strings among
/// its constants (`ParseFrame::kstr[k.0..k.1]`), its first `kids`
/// prototypes, `ups` upvalue names and `locs` local variable names.
pub(crate) struct OpenView {
    pub(crate) proto: *mut GcHeader,
    pub(crate) k: (usize, usize),
    pub(crate) kids: usize,
    pub(crate) ups: usize,
    pub(crate) locs: usize,
}

/// A load in progress while a step runs inside it.
pub(crate) struct ParseFrame {
    /// PUC's parser objects on the loading thread's stack, bottom first
    pub(crate) slots: Vec<*mut GcHeader>,
    /// luna's main closure of the chunk (from 5.2 among `slots` too)
    pub(crate) closure: *mut GcHeader,
    /// the prototypes being built, outermost first
    pub(crate) open: Vec<OpenView>,
    pub(crate) kstr: Vec<*mut GcHeader>,
}

/// Whether `h` heads a parser table.
///
/// # Safety
/// `h` heads an allocated object.
#[inline]
pub(in crate::runtime::puc) unsafe fn is_load_tab(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Ghost && (*h).sub == LOADTAB }
}

/// PUC's size of the parser table behind `h`.
///
/// # Safety
/// `h` heads a parser table.
pub(in crate::runtime::puc) unsafe fn load_tab_size(
    s: &crate::runtime::puc::size::PucSizes,
    d: Dialect,
    h: *mut GcHeader,
) -> usize {
    // SAFETY: the caller's contract
    let t = unsafe { &*(h as *const LoadTab) };
    s.table
        + crate::runtime::puc::work::array_bytes(s, d, t.asize as usize)
        + crate::runtime::puc::work::hash_bytes(s, d, t.nodes as usize)
}

impl Heap {
    /// A new empty parser table (PUC `luaH_new`), counted.
    pub(in crate::runtime) fn new_load_tab(&mut self) -> Result<Gc<LoadTab>, Oom> {
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

    /// The parser table `t` resized to `a` array slots and `h` nodes (PUC
    /// `luaH_resize`).
    pub(in crate::runtime) fn load_tab_resize(&mut self, t: Gc<LoadTab>, a: usize, h: usize) {
        // SAFETY: `t` is a handle, so the parser table is live, and nothing
        // else borrows it
        let lt = unsafe { t.as_mut() };
        let (oa, oh) = (lt.asize as usize, lt.nodes as usize);
        (lt.asize, lt.nodes) = (a as u32, h as u32);
        if lt.counted {
            self.acct_counted_table_resize((oa, oh), (a, h));
        }
    }

    /// `obj` keyed in the parser table `t`: a black table that takes a
    /// white object goes back to gray (`luaC_barrierback` in
    /// `luaH_newkey`).
    pub(in crate::runtime) fn load_tab_ref(
        &mut self,
        t: Gc<LoadTab>,
        obj: Gc<GcHeader>,
    ) -> Result<(), Oom> {
        // SAFETY: `t` is a handle, so the parser table is live, and nothing
        // else borrows it
        unsafe { t.as_mut() }.refs.push(obj.as_ptr())?;
        if is_white(obj.flags()) {
            // SAFETY: `t` is a handle, so the parser table is live
            unsafe { self.barrier_back_obj(t.header()) };
        }
        Ok(())
    }

    /// `obj`'s key in the parser table `t` set to nil: a dead key, which
    /// a traversal does not mark.
    pub(in crate::runtime) fn load_tab_unref(&mut self, t: Gc<LoadTab>, obj: *mut GcHeader) {
        // SAFETY: `t` is a handle, so the parser table is live, and nothing
        // else borrows it
        let refs = unsafe { &mut t.as_mut().refs };
        if let Some(i) = refs.iter().rposition(|&r| r == obj) {
            refs.remove(i);
        }
    }

    /// The work of traversing the parser table behind `h`: a table's of
    /// its sizes.
    ///
    /// # Safety
    /// `h` heads a parser table.
    pub(in crate::runtime::puc) unsafe fn load_tab_work(&self, h: *mut GcHeader) -> i64 {
        // SAFETY: the caller's contract
        let t = unsafe { &*(h as *const LoadTab) };
        let (a, n) = (t.asize as usize, t.nodes as usize);
        let s = &self.puc.sizes;
        (match self.puc.dialect {
            Dialect::L51 | Dialect::L52 => s.table + self.array_bytes(a) + s.node * n.max(1),
            Dialect::L53 => s.table + self.array_bytes(a) + s.node * n,
            Dialect::L54 => 1 + a + 2 * n,
            Dialect::L55 => 1 + 2 * n.max(1) + a,
        }) as i64
    }

    /// A dead parser table freed, PUC's table with it.
    ///
    /// # Safety
    /// `h` heads an unlinked parser table nothing uses afterwards.
    pub(in crate::runtime::puc) unsafe fn free_load_tab(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            // `luaH_free`: the hash part, the array part, the table
            let t = &*(h as *const LoadTab);
            let (a, n, k) = (t.asize as usize, t.nodes as usize, usize::from(t.counted));
            for b in [
                self.hash_bytes(n),
                self.array_bytes(a),
                self.puc.sizes.table,
            ] {
                if b > 0 {
                    self.acct_part(b * k, 0);
                }
            }
            self.free_block(h as *mut LoadTab);
        }
    }
}

/// Mark the object of `g` as one the parser is building, or no more.
pub(crate) fn set_parsing<T: GcObject>(g: Gc<T>, on: bool) {
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

impl Heap {
    /// A dead object a sweep reached: when it is a string only a load held
    /// ([`KEPT`]), it is freed in the count, as PUC frees it, and kept
    /// alive and uncounted, to be counted again where PUC makes it again.
    /// Whether it was.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    #[inline]
    pub(in crate::runtime::puc) unsafe fn kept_dead(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).cls & KEPT == 0 || (*h).tag != ObjTag::Str {
                return (*h).cls & KEPT != 0 && self.keeper_kept(h);
            }
            (*h).cls &= !KEPT;
            self.trace_line(format_args!(
                "kept {}",
                (*(h as *const crate::runtime::LuaStr)).len()
            ));
            // the string sweep took it off the count already
            if (*h).sub & strt::SWEPT != 0 {
                (*h).sub &= !strt::SWEPT;
            } else {
                self.acct_string_free((*(h as *const crate::runtime::LuaStr)).len());
            }
            (*h).set_uncounted(true);
            (*h).paint(self.white());
        }
        true
    }
}

impl Heap {
    /// `luaC_objbarrier` from the main closure `cl` to its new upvalue
    /// `uv`: an upvalue is marked with what it holds at once, never gray.
    pub(in crate::runtime) fn barrier_new_upval(
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
}
