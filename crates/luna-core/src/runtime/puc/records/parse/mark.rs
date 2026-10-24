//! Marking what a load in progress holds while a collection step runs
//! inside it: its parser tables, the prototypes being built as PUC has
//! them so far, and what luna's own objects of the load need.

use super::*;
use super::{KEPT, LoadTab, ParseFrame};

impl Marker {
    /// Traverse the parser table `h`.
    ///
    /// # Safety
    /// `h` heads a parser table, whose objects are allocated.
    pub(in crate::runtime) unsafe fn trace_load_tab(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            for &r in (*(h as *const LoadTab)).refs.iter() {
                self.header(r);
            }
        }
    }

    /// Traverse `p` as PUC traverses the prototype it is building
    /// (`traverseproto`): its source, its constants, upvalue names,
    /// prototypes and local variable names so far. False when `p` is no
    /// prototype being built.
    pub(crate) fn trace_parsing_proto(&mut self, p: &crate::runtime::Proto) -> bool {
        let me = p as *const crate::runtime::Proto as *mut GcHeader;
        let frames = self.load_frames() as *const [ParseFrame];
        // SAFETY: the frames live in the heap; marking does not change them
        for f in unsafe { &*frames }.iter().rev() {
            let Some(v) = f.open.iter().find(|v| v.proto == me) else {
                continue;
            };
            self.mark(p.source);
            for &s in &f.kstr[v.k.0..v.k.1] {
                // SAFETY: a constant of a prototype being built is allocated
                unsafe { self.header(s) };
            }
            let names = p.puc_names.borrow();
            let nloc = p.locvars.len();
            for &s in names.iter().skip(nloc).take(v.ups) {
                self.mark(s);
            }
            for &c in p.protos.iter().take(v.kids) {
                self.mark(c);
            }
            for &s in names.iter().take(v.locs.min(nloc)) {
                self.mark(s);
            }
            return true;
        }
        false
    }

    /// Traverse `c` as PUC traverses the main closure of a chunk being
    /// parsed: its prototype, no upvalue yet. False when `c` is no such
    /// closure.
    pub(crate) fn trace_parsing_closure(&mut self, c: &crate::runtime::LuaClosure) -> bool {
        let me = c as *const crate::runtime::LuaClosure as *mut GcHeader;
        if !self.load_frames().iter().any(|f| f.closure == me) {
            return false;
        }
        self.mark(c.proto);
        true
    }

    /// Everything the loads in progress hold for luna, whatever has been
    /// marked: each chunk's closure with its upvalues, and each prototype
    /// of its whole tree, made by PUC yet or not, marked without counting
    /// (left so: the caller drains what this leaves gray). A string of
    /// PUC's no object of PUC's holds is left white, [`KEPT`]: PUC frees
    /// it. False, with nothing done, when no load is in progress.
    pub(in crate::runtime) fn quiet_parse_trees(&mut self) -> bool {
        if self.all_load_frames().is_empty() {
            return false;
        }
        self.set_quiet(true);
        let frames = self.all_load_frames() as *const [ParseFrame];
        // SAFETY: the frames live in the heap; marking does not change them
        for f in unsafe { &*frames } {
            if f.closure.is_null() {
                continue;
            }
            // SAFETY: a frame's closure is allocated while its load runs
            let cl = unsafe { &*(f.closure as *const crate::runtime::LuaClosure) };
            // SAFETY: as above
            unsafe { self.header(f.closure) };
            for &uv in cl.upvals().iter() {
                self.mark(uv);
            }
            let mut todo = vec![cl.proto];
            while let Some(p) = todo.pop() {
                let h = p.as_ptr() as *mut GcHeader;
                // SAFETY: a prototype of a load in progress is allocated;
                // it is traversed here, so it goes black at once
                unsafe { (*h).paint(BLACK) };
                self.keep(p.source);
                for &k in p.consts.iter() {
                    match k {
                        Value::Str(s) => self.keep(s),
                        k => {
                            self.value(k);
                        }
                    }
                }
                for &s in p.puc_names.borrow().iter() {
                    self.keep(s);
                }
                for &q in p.inlined_protos.borrow().iter() {
                    self.mark(q);
                }
                todo.extend(p.protos.iter().copied());
            }
        }
        true
    }

    /// A string luna holds and PUC may not (a load's prototype's, luna's
    /// own names): marked, or left white and [`KEPT`] when it is PUC's and
    /// nothing of PUC's marked it.
    pub(crate) fn keep(&mut self, s: Gc<crate::runtime::LuaStr>) {
        let h = s.header();
        // SAFETY: a string luna holds is allocated; only its header is read
        // and written
        unsafe {
            if is_white((*h).flags()) && !(*h).is_uncounted() && (*h).sub & EXTERNAL == 0 {
                (*h).cls |= KEPT;
                return;
            }
        }
        self.mark(s);
    }
}

impl Heap {
    /// PUC's `luaC_barrier` as the parser puts `child` in `parent`: while
    /// marking, a black parent's white child is marked; in a sweep, a black
    /// parent not swept yet goes white instead (and nothing waits on a gray
    /// list for a cycle that has not started).
    ///
    /// # Safety
    /// Both head allocated objects of this heap.
    pub(crate) unsafe fn parse_barrier(&mut self, parent: *mut GcHeader, child: *mut GcHeader) {
        // SAFETY: the caller's contract; only flag bytes are read and written
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
}

impl Heap {
    /// The main closure `cl` of a load is built: when a step of this cycle
    /// traversed it as PUC had it (no upvalue yet), its upvalues are marked
    /// too.
    pub(crate) fn settle_closure(&mut self, cl: Gc<crate::runtime::LuaClosure>) {
        for &uv in cl.upvals().iter() {
            self.barrier_new_upval(cl, uv);
        }
    }
}
