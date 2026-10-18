//! The incremental cycle: propagating, the atomic step, and the work
//! PUC's collector counts for each.

use super::*;

#[path = "heap_atomic.rs"]
mod atomic;
#[path = "heap_sizer.rs"]
mod sizer;
pub(crate) use sizer::Sizer;

impl Heap {
    /// Begin an incremental mark cycle (precondition: `Pause`): enter
    /// `GcPhase::Propagate` with a fresh PropagateState and lend out the
    /// marker, with tobefnz and any barrier-carried gray already in it, for
    /// the caller to mark its roots with and return through
    /// [`Heap::stash_marker`].
    pub(crate) fn gc_begin_propagate(&mut self) -> Marker {
        debug_assert!(self.phase == GcPhase::Pause);
        self.phase = GcPhase::Propagate;
        self.env51_whiten();
        // 5.2's fixed strings are white like any other: a cycle marks them
        // where it reaches them, and counts them
        if self.table_dialect == crate::runtime::table::Dialect::L52 {
            self.color_fixed_strings(self.current_white);
        }
        // what a back barrier sent there during the last sweep is white now
        self.grayagain = mark::GrayStack::default();
        self.propagate = Some(PropagateState {
            quiet: Vec::new(),
            weak: Vec::new(),
            ephemeron: Vec::new(),
            cached_protos: Vec::new(),
            no_ephemeron: self.no_ephemeron,
        });
        self.loan_marker()
    }

    /// Give the fixed strings the color `c` (0: gray, never marked).
    fn color_fixed_strings(&mut self, c: u8) {
        let mut cur = self.fixed;
        while !cur.is_null() {
            // SAFETY: the fixed list links live objects, freed only with the
            // heap; only the flag byte is written
            unsafe {
                if (*cur).tag == ObjTag::Str {
                    (*cur).flags = (*cur).with_slow(((*cur).flags & !COLOR_BITS) | c);
                }
                cur = (*cur).next;
            }
        }
    }

    /// Mark the objects awaiting their finalizers (PUC `markbeingfnz`,
    /// 5.1's `marktmu`).
    pub(crate) fn mark_tobefnz(&self, m: &mut Marker) {
        for &h in self.tobefnz_live() {
            // SAFETY: a queued finalizable stays allocated until its `__gc` has run (`take_one_tobefnz`)
            unsafe { m.header(h) };
        }
    }

    /// [`Heap::gc_begin_propagate`] with `roots` / `extra` as the roots.
    #[cfg(test)]
    pub(crate) fn gc_start_propagate(&mut self, roots: &[Value], extra: &[Gc<Upvalue>]) {
        let mut m = self.gc_begin_propagate();
        for &r in roots {
            m.value(r);
        }
        for &uv in extra {
            m.mark(uv);
        }
        self.stash_marker(m);
    }

    /// Drain up to `budget` gray objects (blacken + trace). Returns true if
    /// the gray queue is now empty (caller should follow up with
    /// `gc_finish_atomic`). PUC `propagatemark` budgeted loop.
    #[cfg(test)]
    pub(crate) fn gc_step_propagate(&mut self, budget: usize) -> bool {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        let mut n = 0;
        while n < budget {
            let Some(h) = m.pop() else {
                break;
            };
            // SAFETY: `h` was popped off the gray stack, which only `Marker::header` and `barrier_back` push to, with headers of allocated objects; frees happen only in the sweep, never during propagate or between its steps
            unsafe { blacken(h, &mut m) };
            n += 1;
        }
        let exhausted = m.stack.is_empty() && !m.overflow;
        self.stash_marker(m);
        exhausted
    }

    /// Phase peek (for the VM-side step driver).
    pub(crate) fn gc_phase_is_pause(&self) -> bool {
        self.phase == GcPhase::Pause
    }
    pub(crate) fn gc_phase_is_propagate(&self) -> bool {
        self.phase == GcPhase::Propagate
    }
    #[allow(dead_code)] // public phase-peek API trio; sweep variant unused internally
    pub(crate) fn gc_phase_is_sweep(&self) -> bool {
        self.phase == GcPhase::Sweep
    }
}

/// Where a run of [`Heap::propagate_work`] stopped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Propagated {
    /// no object is gray
    Empty,
    /// the work it was given is done
    Budget,
    /// it traversed this thread, whose values the VM holds (the running
    /// one, or the main one while a coroutine runs)
    Thread(*mut GcHeader),
    /// 5.3: the thread it is about to traverse, gray again, which the
    /// collector shrinks first (`luaD_shrinkstack` in `traversethread`)
    Shrink(*mut GcHeader),
}

/// The running thread, for the work of traversing it: its object, and
/// the top of its stack, the stack's size and its `CallInfo`s (PUC's
/// counts), which the VM holds while it runs; and the same of the main
/// thread while a coroutine runs (null else), which the VM holds too.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Running {
    pub(crate) h: *mut GcHeader,
    pub(crate) top: u32,
    pub(crate) size: u32,
    pub(crate) ci: u32,
    pub(crate) main: *mut GcHeader,
    pub(crate) main_top: u32,
    pub(crate) main_size: u32,
    pub(crate) main_ci: u32,
}

impl Heap {
    /// 5.2 and 5.3 count the strings and userdata a traversal marks.
    pub(super) fn leaf_hdr(&self) -> usize {
        match self.table_dialect {
            crate::runtime::table::Dialect::L52 | crate::runtime::table::Dialect::L53 => {
                self.sizes.str_hdr
            }
            _ => 0,
        }
    }

    /// 5.2 counts a closed upvalue a traversal marks.
    pub(super) fn leaf_upval(&self) -> usize {
        match self.table_dialect {
            crate::runtime::table::Dialect::L52 => self.sizes.upval,
            _ => 0,
        }
    }

    /// PUC `propagatemark` one object at a time, taking each one's work off
    /// `remaining`: the first always (a step's loop runs its body once
    /// before testing), the next ones while `remaining` is above `floor`.
    /// Returns whether it looked for one more and found none.
    pub(crate) fn propagate_work(
        &mut self,
        remaining: &mut i64,
        floor: i64,
        run: Running,
    ) -> Propagated {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        let mut out = Propagated::Budget;
        loop {
            let Some(h) = m.pop() else {
                out = Propagated::Empty;
                break;
            };
            // SAFETY: a popped header heads an allocated object
            let coro = unsafe { (*h).tag } == ObjTag::Coro;
            if self.table_dialect == crate::runtime::table::Dialect::L53
                && coro
                && !self.puc.shrunk.contains(&h)
            {
                self.puc.shrunk.push(h);
                m.push_gray(h);
                out = Propagated::Shrink(h);
                break;
            }
            m.leaf_bytes = 0;
            // SAFETY: `h` came off the gray stack, which holds headers of
            // allocated objects; nothing is freed while marking
            unsafe {
                blacken(h, &mut m);
                let w = self.object_work(h, run) + m.leaf_bytes;
                #[cfg(feature = "gc-trace")]
                self.trace_prop(h, w, run);
                *remaining -= w;
            }
            // a thread whose values the VM holds: the VM marks them next
            if h == run.h || h == run.main {
                out = Propagated::Thread(h);
                break;
            }
            if *remaining <= floor {
                break;
            }
        }
        self.stash_marker(m);
        out
    }

    /// `R38_PROP`: the object `h` traversed with work `w`.
    #[cfg(feature = "gc-trace")]
    pub(super) unsafe fn trace_prop(&mut self, h: *mut GcHeader, w: i64, run: Running) {
        // SAFETY: the caller's contract: `h` heads an allocated object
        unsafe {
            if !self.trace.prop {
                return;
            }
            {
                let (a, n) = match (*h).tag {
                    ObjTag::Table => {
                        let t = &*(h as *const Table);
                        (t.asize as usize, t.node_count())
                    }
                    ObjTag::Coro if h == run.h => (run.size as usize, run.ci as usize),
                    ObjTag::Coro if h == run.main => (run.main_size as usize, run.main_ci as usize),
                    ObjTag::Coro => {
                        let c = &*(h as *const crate::runtime::Coro);
                        (c.stack_size as usize, c.ci_alloc as usize)
                    }
                    _ => (0, 0),
                };
                let mut k = String::from("-");
                if (*h).tag == ObjTag::Table {
                    let t = &*(h as *const Table);
                    if let Some(s) = t.first_str_key() {
                        k = String::from_utf8_lossy(s.as_bytes()).into_owned();
                    }
                } else if (*h).tag == ObjTag::Native {
                    let n = &*(h as *const crate::runtime::NativeClosure);
                    k = format!("{:p}", n.f as *const ());
                } else if (*h).tag == ObjTag::Userdata {
                    let u = &*(h as *const crate::runtime::Userdata);
                    let kind = match &u.payload {
                        crate::runtime::userdata::UserdataPayload::File(_) => "file",
                        crate::runtime::userdata::UserdataPayload::Empty => "empty",
                        _ => "host",
                    };
                    k = format!("{kind}/{}/{}", u.puc_size, is_uncounted(h));
                }
                // a record of 5.1's environments as the table or closure
                // it stands for
                let mut tag = (*h).tag as u8;
                if super::env51::is_phantom(h) {
                    let t = super::env51::phantom_is_table(h);
                    tag = if t { ObjTag::Table } else { ObjTag::Native } as u8;
                    k = String::from("env51");
                }
                self.trace.line(format_args!("p {tag} {w} {a} {n} {k}"));
            }
        }
    }

    /// A sweep with nothing dead, as 5.5's `minor2inc` starts one: the
    /// objects go back to the current white.
    pub(crate) fn gc_sweep_all_live(&mut self) {
        debug_assert!(self.phase == GcPhase::Pause);
        self.phase = GcPhase::Sweep;
        self.sweep_prev = ptr::null_mut();
        self.sweep_objects(usize::MAX, true);
    }

    /// No object is gray.
    pub(crate) fn gray_is_empty(&self) -> bool {
        self.gray.is_empty() && !self.gray_overflow
    }
}

impl Marker {
    /// The work the strings and userdata marked so far add (5.2, 5.3),
    /// counted from 0 again.
    pub(crate) fn take_leaf_bytes(&mut self) -> i64 {
        std::mem::take(&mut self.leaf_bytes)
    }
}
