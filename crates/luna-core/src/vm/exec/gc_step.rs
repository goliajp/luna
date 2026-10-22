//! What PUC's collector steps (`runtime::puc::step`) need of the VM: the
//! threads it holds and their stacks, the marking from its roots, the
//! finalizers it runs.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::heap::{Propagated, Running};
use crate::runtime::puc::step::StepHost;

impl StepHost for Vm {
    #[inline]
    fn heap(&mut self) -> &mut crate::runtime::Heap {
        &mut self.heap
    }
    fn running(&self) -> Running {
        self.gc_running()
    }
    fn before_step(&mut self) {
        self.jit_forget_ghosts();
        self.name_cache = Default::default();
    }
    fn propagate(&mut self, work: &mut i64, floor: i64, run: Running) -> bool {
        self.gc_propagate(work, floor, run)
    }
    fn restart(&mut self) {
        self.gc_restart();
    }
    fn atomic(&mut self) -> (i64, i64) {
        self.gc_atomic()
    }
    fn sweep_quota(&mut self, quota: i64) -> Option<(i64, i64)> {
        Vm::sweep_quota(self, quota)
    }
    fn run_finalizer(&mut self, propagate: bool) -> Result<(), LuaError> {
        self.gc_tm(propagate)
    }
    fn finalizer_has_room(&mut self, closing: bool) -> bool {
        Vm::finalizer_has_room(self, closing)
    }
    fn shrink_swept_thread_52(&mut self, h: *mut crate::runtime::heap::GcHeader) {
        if !self.main_coro.is_some_and(|m| m.as_ptr() as *mut _ == h) {
            self.collector_shrinks_thread(h);
        }
    }
    fn sweep_main_52(&mut self) {
        Vm::sweep_main_52(self);
    }
}

impl Vm {
    /// The end of 5.2's sweep: the main thread, swept last, is shrunk
    /// (`sweepthread`).
    pub(crate) fn sweep_main_52(&mut self) {
        if self.current.is_none() {
            let inuse = self.stack_in_use();
            self.shrink_running(crate::vm::exec::stack_inuse::Shrink::Collector, inuse);
        } else {
            self.collector_shrinks_main(crate::vm::exec::stack_inuse::Shrink::Collector);
        }
    }

    /// The running thread, whose stack the VM holds.
    pub(super) fn gc_running(&self) -> Running {
        let co = self.current.or(self.main_coro);
        let main = match (&self.main_ctx, self.main_coro) {
            (Some(m), Some(mc)) => {
                let ci = match self.heap.dialect() {
                    Dialect::L51 => m.frame_size,
                    // the levels below its running one, as for the running
                    // thread ([`Vm::ci_in_use`])
                    Dialect::L52 => {
                        let levels = (m.frames.len() + m.natives.len()) as u32 + self.g.host_frames;
                        levels - m.meta_conts - m.host_conts
                    }
                    _ => m.ci_alloc,
                };
                Some((mc.as_ptr() as *mut _, m.top, m.stack_size, ci))
            }
            _ => None,
        };
        let (main, main_top, main_size, main_ci) = main.unwrap_or((std::ptr::null_mut(), 0, 0, 0));
        Running {
            h: co.map_or(std::ptr::null_mut(), |c| c.as_ptr() as *mut _),
            // with a load's parser objects above
            top: self.gc_top + self.heap.load_slots(),
            size: self.stack_size,
            // 5.1 counts its `CallInfo` array, 5.2 the ones in use below
            // the running one, the later dialects all it has
            ci: match self.heap.dialect() {
                Dialect::L51 => self.g.frame_size,
                Dialect::L52 => self.ci_in_use(),
                _ => self.g.ci_alloc,
            },
            main,
            main_top,
            main_size,
            main_ci,
        }
    }

    /// PUC's propagation with `work` to do (down to `floor`): when it
    /// traverses a thread whose values the VM holds, the VM marks them and
    /// it goes on. True when no object is gray.
    pub(super) fn gc_propagate(&mut self, work: &mut i64, floor: i64, mut run: Running) -> bool {
        loop {
            match self.heap.propagate_work(work, floor, run) {
                Propagated::Empty => return true,
                Propagated::Budget => return false,
                Propagated::Shrink(h) => {
                    self.collector_shrinks_thread(h);
                    run = self.gc_running();
                }
                Propagated::Thread(h) => {
                    let mut m = self.heap.loan_marker();
                    self.mark_thread_values(&mut m, h);
                    // what the thread's stack marks is its traversal's work
                    *work -= m.take_leaf_bytes();
                    self.heap.stash_marker(m);
                    if *work <= floor {
                        return false;
                    }
                }
            }
        }
    }

    /// The collector visiting thread `h` as it traverses it (5.3).
    pub(super) fn collector_shrinks_thread(&mut self, h: *mut crate::runtime::heap::GcHeader) {
        let co = self.current.or(self.main_coro);
        if co.is_some_and(|c| c.as_ptr() as *mut _ == h) {
            let inuse = self.stack_in_use();
            self.shrink_running(super::stack_inuse::Shrink::Collector, inuse);
        } else if self.main_ctx.is_some()
            && self.main_coro.is_some_and(|c| c.as_ptr() as *mut _ == h)
        {
            self.collector_shrinks_main(crate::vm::exec::stack_inuse::Shrink::Collector);
        } else {
            // SAFETY: `h`, gray, heads a live coroutine
            let co = unsafe { crate::runtime::heap::Gc::from_ptr(h as *mut _) };
            self.collector_shrinks_coro(co, crate::vm::exec::stack_inuse::Shrink::Collector);
        }
    }

    /// `restartcollection` (5.1 `markroot`): the roots marked, the cycle
    /// propagating.
    pub(super) fn gc_restart(&mut self) {
        let mut m = self.heap.gc_begin_propagate();
        self.mark_roots_restart(&mut m);
        // 5.1 marks them in its atomic step
        if self.version != LuaVersion::Lua51 {
            self.heap.mark_tobefnz(&mut m);
        }
        self.heap.stash_marker(m);
    }

    /// PUC `atomic` and `entersweep`; returns the work and how many
    /// objects the sweep went over on the way in.
    pub(super) fn gc_atomic(&mut self) -> (i64, i64) {
        self.clear_dead_stack(true);
        let mut run = self.gc_running();
        // 5.2's final traversal of a thread counts no `CallInfo`
        if self.heap.dialect() == Dialect::L52 {
            run.ci = 0;
            run.main_ci = 0;
        }
        let mut m = self.heap.loan_marker();
        if self.version == LuaVersion::Lua51 {
            self.heap.mark_tobefnz(&mut m);
        }
        // 5.2 and 5.3 count what the roots other than the threads reach
        // (a basic type's metatable made meanwhile); the threads, gray
        // again, are traversed with `grayagain`, uncounted
        let roots = if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            self.mark_state_roots(&mut m, true);
            let w = self.heap.drain_work(&mut m, Some(run));
            self.mark_threads_held(&mut m);
            w
        } else {
            self.mark_roots(&mut m, true);
            0
        };
        self.heap.stash_marker(m);
        self.watch_errors();
        self.lend_open_lists();
        let (work, swept) = self.heap.gc_finish_atomic_work(Some(run));
        let r = (work + roots, swept);
        // 5.3 shrank them as it traversed them
        if self.version != LuaVersion::Lua53 {
            self.collector_shrinks_threads();
        }
        self.forget_dead_errors();
        // there may be objects to be finalized
        self.heap.finalizers_due();
        r
    }

    /// The list sweep of a state that sweeps `quota` objects: how many
    /// it swept and the bytes it freed; `None` once the list is done.
    pub(super) fn sweep_quota(&mut self, quota: i64) -> Option<(i64, i64)> {
        if !self.heap.gc_phase_is_sweep() {
            return None;
        }
        let before = self.heap.bytes() as i64;
        let (n, _) = self.heap.sweep_counted(quota);
        Some((n, before - self.heap.bytes() as i64))
    }
}
