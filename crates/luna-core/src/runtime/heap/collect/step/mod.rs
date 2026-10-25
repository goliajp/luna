//! The incremental cycle: propagating, the atomic step, and the work
//! PUC's collector counts for each.

use super::*;

mod atomic;
pub(crate) use crate::runtime::puc::Sizer;

impl Heap {
    /// Begin an incremental mark cycle (precondition: `Pause`): enter
    /// `GcPhase::Propagate` with a fresh PropagateState and lend out the
    /// marker, with tobefnz and any barrier-carried gray already in it, for
    /// the caller to mark its roots with and return through
    /// [`Heap::stash_marker`].
    pub(crate) fn gc_begin_propagate(&mut self) -> Marker {
        debug_assert!(self.phase == GcPhase::Pause);
        self.phase = GcPhase::Propagate;
        self.puc_mark_begins();
        // 5.2's fixed strings are white like any other: a cycle marks them
        // where it reaches them, and counts them
        if self.puc_rules().fixed_strings_marked {
            self.color_fixed_strings(self.current_white);
        }
        // what a back barrier sent there during the last sweep is white now
        self.grayagain = mark::GrayStack::default();
        self.propagate = Some(PropagateState {
            quiet: Vec::new(),
            weak: Vec::new(),
            ephemeron: Vec::new(),
            no_ephemeron: !self.rules.ephemeron(),
            atomic: false,
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

    /// Mark the objects awaiting their finalizers (`markbeingfnz`, `marktmu`).
    pub(crate) fn mark_tobefnz(&self, m: &mut Marker) {
        let mut h = self.tobefnz;
        // SAFETY: `tobefnz` links allocated objects
        unsafe {
            while !h.is_null() {
                m.header(h);
                h = (*h).next;
            }
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

    /// PUC `luaC_fullgc` with marking under way (`keepinvariant`): no
    /// atomic step, `entersweep` and the sweep to its end (5.1 back to the
    /// string sweep), which with the white not flipped frees nothing and
    /// turns every object white again, sweeping the threads as it does.
    /// The marking done so far is dropped; the collection starts over.
    pub(crate) fn gc_abandon_marking(&mut self) {
        let mut m = self.loan_marker();
        self.settle_sizer(&mut m);
        while m.stack.pop().is_some() {}
        self.gray = m.stack;
        self.gray_overflow = false;
        self.grayagain = super::super::mark::GrayStack::default();
        // the sweep, keeping every object: the white has not flipped
        self.phase = GcPhase::Sweep;
        self.sweep_begins();
        let white = self.current_white;
        // SAFETY: the lists link allocated objects, none freed here; only
        // headers are written and the model's hooks of a kept object run
        unsafe {
            let mut cur = self.all;
            while !cur.is_null() {
                self.puc_sweep_reach(cur);
                (*cur).flags = (*cur).with_slow(((*cur).flags & !COLOR_BITS) | white);
                self.puc_sweep_kept(cur);
                cur = (*cur).next;
            }
        }
        self.whiten_fin_lists();
        self.puc_sweep_ends();
        self.phase = GcPhase::Pause;
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
#[derive(Clone, Copy)]
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
    Shrink(Gc<crate::runtime::Coro>),
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
            if coro
                && self.puc_rules().shrink_at == crate::runtime::puc::for_heap::ShrinkAt::Traverse
                && self.puc_shrinks_now(h)
            {
                m.push_gray(h);
                // SAFETY: `h` came off the gray stack, so it heads an
                // allocated object, and it is a coroutine
                out = Propagated::Shrink(unsafe { Gc::from_ptr_unchecked(h.cast()) });
                break;
            }
            m.leaf_bytes = 0;
            // SAFETY: `h` came off the gray stack, which holds headers of
            // allocated objects; nothing is freed while marking
            unsafe {
                blacken(h, &mut m);
                let w = self.object_work(h, run, &m.lent.stands) + m.leaf_bytes;
                self.hook_prop(h, w, run);
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

    /// A sweep with nothing dead, as 5.5's `minor2inc` starts one: the
    /// objects go back to the current white.
    pub(crate) fn gc_sweep_all_live(&mut self) {
        debug_assert!(self.phase == GcPhase::Pause);
        self.phase = GcPhase::Sweep;
        self.sweep_prev = ptr::null_mut();
        self.sweep_objects(usize::MAX, true);
    }

    /// 5.2 `luaC_changemode` to incremental: drop any marking, sweep nothing.
    pub(crate) fn gc_sweep_unmarked_52(&mut self) {
        if self.phase == GcPhase::Propagate {
            self.propagate = None;
            let _ = self.gray.take();
            self.gray_overflow = false;
            self.grayagain = mark::GrayStack::default();
            self.color_fixed_strings(0);
            self.phase = GcPhase::Pause;
        }
        self.collect_dead_strings();
        self.gc_sweep_all_live();
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
