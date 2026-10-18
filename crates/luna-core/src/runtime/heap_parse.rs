//! What a load's parser holds while a collection step runs in the middle
//! of the load (PUC checks for a collection as it parses): its tables,
//! records of PUC's on the object list ([`LoadTab`]), and for each load in
//! progress a frame naming the objects on the loading thread's stack and
//! what each prototype being built holds so far ([`ParseFrame`]).

use super::*;
use crate::runtime::mem::{LVec, Oom};
use crate::runtime::table::Dialect;

#[path = "heap_parse_mark.rs"]
mod mark;
#[path = "heap_parse_unborn.rs"]
mod unborn;

/// In a record's `sub` byte: a parser table.
const LOADTAB: u8 = 0x20;
/// In a prototype's or a closure's `cls`: the parser is building it.
pub(crate) const PARSING: u8 = 0x20;
/// In a string's `cls`: only a load in progress holds it, which PUC has
/// not made it for yet. PUC frees it; luna keeps it, uncounted.
pub(in crate::runtime::heap) const KEPT: u8 = 0x20;

/// [`KEPT`] for a string, nothing for another object (whose bit means
/// something else).
#[inline(always)]
pub(in crate::runtime::heap) fn kept_bit(tag: ObjTag) -> u8 {
    if tag == ObjTag::Str { KEPT } else { 0 }
}

/// One of PUC's parser tables, which luna has no table for: its parts'
/// sizes and the objects among its keys and values.
#[repr(C)]
pub(crate) struct LoadTab {
    pub(crate) hdr: GcHeader,
    asize: u32,
    nodes: u32,
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
pub(in crate::runtime::heap) unsafe fn is_load_tab(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Ghost && (*h).sub == LOADTAB }
}

/// PUC's size of the parser table behind `h`.
///
/// # Safety
/// `h` heads a parser table.
pub(in crate::runtime::heap) unsafe fn load_tab_size(
    s: &crate::runtime::puc_size::PucSizes,
    d: Dialect,
    h: *mut GcHeader,
) -> usize {
    // SAFETY: the caller's contract
    let t = unsafe { &*(h as *const LoadTab) };
    s.table + pace::array_bytes(s, d, t.asize as usize) + pace::hash_bytes(s, d, t.nodes as usize)
}

impl Heap {
    /// A new empty parser table (PUC `luaH_new`), counted.
    pub(crate) fn new_load_tab(&mut self) -> Result<*mut GcHeader, Oom> {
        let mut hdr = GcHeader::new(ObjTag::Ghost);
        (hdr.sub, hdr.flags, hdr.cls) = (LOADTAB, 0, 0);
        let refs = LVec::new(self.mem());
        let t = self.try_adopt(LoadTab {
            hdr,
            asize: 0,
            nodes: 0,
            refs,
        })?;
        self.acct_obj(PucKind::Table, self.sizes.table);
        Ok(t.as_ptr() as *mut GcHeader)
    }

    /// The parser table `t` resized to `a` array slots and `h` nodes (PUC
    /// `luaH_resize`).
    pub(crate) fn load_tab_resize(&mut self, t: *mut GcHeader, a: usize, h: usize) {
        // SAFETY: `t` heads a live parser table, and nothing else borrows it
        let lt = unsafe { &mut *(t as *mut LoadTab) };
        let (oa, oh) = (lt.asize as usize, lt.nodes as usize);
        (lt.asize, lt.nodes) = (a as u32, h as u32);
        self.acct_table_resize(oa, oh, a, h);
    }

    /// `obj` keyed in the parser table `t`: a black table that takes a
    /// white object goes back to gray (`luaC_barrierback` in
    /// `luaH_newkey`).
    pub(crate) fn load_tab_ref(&mut self, t: *mut GcHeader, obj: *mut GcHeader) -> Result<(), Oom> {
        // SAFETY: `t` heads a live parser table and `obj` a live object
        unsafe {
            (*(t as *mut LoadTab)).refs.push(obj)?;
            if is_white((*obj).flags) {
                self.barrier_back_obj(t);
            }
        }
        Ok(())
    }

    /// `obj`'s key in the parser table `t` set to nil: a dead key, which
    /// a traversal does not mark.
    pub(crate) fn load_tab_unref(&mut self, t: *mut GcHeader, obj: *mut GcHeader) {
        // SAFETY: `t` heads a live parser table, and nothing else borrows it
        let refs = unsafe { &mut (*(t as *mut LoadTab)).refs };
        if let Some(i) = refs.iter().rposition(|&r| r == obj) {
            refs.remove(i);
        }
    }

    /// The work of traversing the parser table behind `h`: a table's of
    /// its sizes.
    ///
    /// # Safety
    /// `h` heads a parser table.
    pub(in crate::runtime::heap) unsafe fn load_tab_work(&self, h: *mut GcHeader) -> i64 {
        // SAFETY: the caller's contract
        let t = unsafe { &*(h as *const LoadTab) };
        let (a, n) = (t.asize as usize, t.nodes as usize);
        let s = &self.sizes;
        (match self.table_dialect {
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
    pub(in crate::runtime::heap) unsafe fn free_load_tab(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            // `luaH_free`: the hash part, the array part, the table
            let t = &*(h as *const LoadTab);
            let (a, n) = (t.asize as usize, t.nodes as usize);
            for b in [self.hash_bytes(n), self.array_bytes(a), self.sizes.table] {
                if b > 0 {
                    self.acct_free(b);
                }
            }
            self.free_block(h as *mut LoadTab);
        }
    }

    /// The loads in progress, for a marker.
    pub(in crate::runtime::heap) fn parse_ptr(&self) -> *const Vec<ParseFrame> {
        &self.puc.parse
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
    pub(in crate::runtime::heap) unsafe fn kept_dead(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).cls & KEPT == 0 || (*h).tag != ObjTag::Str {
                return false;
            }
            (*h).cls &= !KEPT;
            #[cfg(feature = "gc-trace")]
            self.trace.line(format_args!(
                "kept {}",
                (*(h as *const crate::runtime::LuaStr)).len()
            ));
            // the string sweep took it off the count already
            if (*h).sub & strt::SWEPT != 0 {
                (*h).sub &= !strt::SWEPT;
            } else {
                self.acct_string_free((*(h as *const crate::runtime::LuaStr)).len());
            }
            set_uncounted(h, true);
            let f = (*h).flags;
            (*h).flags = (*h).with_slow((f & !COLOR_BITS) | self.current_white);
        }
        true
    }
}

impl Heap {
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
            if !is_black((*c).flags) || !is_white((*u).flags) {
                return;
            }
            // in a sweep the closure goes white instead (`luaC_barrier`)
            if self.phase != GcPhase::Propagate {
                let f = (*c).flags;
                (*c).flags = (*c).with_slow((f & !COLOR_BITS) | self.current_white);
                return;
            }
            if self.pace.count_marked && self.phase == GcPhase::Propagate {
                self.count_marked(u);
            }
            (*u).flags = (*u).with_slow(((*u).flags & !COLOR_BITS) | BLACK);
        }
        if let crate::runtime::function::UpvalState::Closed(v) = uv.state {
            self.barrier_forward(uv, v);
        }
    }
}

/// Whether `h` is a string only a load holds ([`KEPT`]), which the sweep
/// keeps.
///
/// # Safety
/// `h` heads an allocated object.
#[cfg(feature = "gc-verify")]
pub(in crate::runtime::heap) unsafe fn is_kept(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Str && (*h).cls & KEPT != 0 }
}

impl Heap {
    /// `gc-verify`: a string marked [`KEPT`] is one PUC frees in the sweep
    /// under way and luna keeps: a string of PUC's still counted, dead to
    /// this cycle (unmarked, not the current white), on the list the sweep
    /// has still to reach. The sweep turns it into an uncounted string of
    /// luna's and drops the mark; a marking that reaches it first drops the
    /// mark too. Panics on the first that is anything else.
    #[cfg(feature = "gc-verify")]
    pub(in crate::runtime::heap) fn verify_kept(&self, ctx: &str) {
        let mut cur = self.all;
        // SAFETY: the links of `all` lead to live objects; only headers are
        // read
        unsafe {
            while !cur.is_null() {
                let (tag, f, cls) = ((*cur).tag, (*cur).flags, (*cur).cls);
                if tag == ObjTag::Str && cls & KEPT != 0 {
                    let dead = is_white(f) && f & self.current_white == 0;
                    assert!(
                        dead && !is_uncounted(cur) && self.phase == GcPhase::Sweep,
                        "[gc-verify] {ctx}: kept string {cur:p}: flags {f:#x}, uncounted {}, phase {:?}",
                        is_uncounted(cur),
                        self.phase
                    );
                }
                cur = (*cur).next;
            }
        }
    }
}
