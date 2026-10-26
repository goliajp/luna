//! What PUC's collector steps (`runtime::puc::step`) need of the VM: the
//! threads it holds and their stacks, the marking from its roots, the
//! finalizers it runs.

use super::*;
use crate::runtime::heap::{Marker, Propagated, Running};
use crate::runtime::puc::api::{PucCollector, PucLoads};
use crate::runtime::puc::step::{CiCount, StepHost};

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
    fn mark_roots_restart(&mut self, m: &mut Marker) {
        Vm::mark_roots_restart(self, m);
    }
    fn mark_roots(&mut self, m: &mut Marker, again: bool) {
        Vm::mark_roots(self, m, again);
    }
    fn mark_state_roots(&mut self, m: &mut Marker, again: bool) {
        Vm::mark_state_roots(self, m, again);
    }
    fn mark_threads_held(&mut self, m: &mut Marker) {
        Vm::mark_threads_held(self, m);
    }
    fn marking_ends(&mut self) {
        self.watch_errors();
        self.lend_open_lists();
    }
    fn collection_ends(&mut self) {
        self.take_back_open_lists();
        self.forget_dead_errors();
    }
    fn clear_dead_stack(&mut self) {
        Vm::clear_dead_stack(self);
    }
    fn shrink_stacks(&mut self) {
        self.collector_shrinks_stacks();
    }
    fn shrink_threads(&mut self) {
        self.collector_shrinks_threads();
    }
    fn finalizing(&self) -> bool {
        self.gc_finalizing
    }
    fn shrink_threads_emergency(&mut self) {
        self.emergency_shrinks_threads();
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
    fn shrink_swept_thread_52(&mut self, co: Gc<crate::runtime::Coro>) {
        if !self.main_coro.is_some_and(|m| m.ptr_eq(co)) {
            self.collector_shrinks_thread(co);
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
    pub(in super::super) fn gc_running(&self) -> Running {
        let co = self.current.or(self.main_coro);
        let count = self.heap.puc_ci_count();
        let main = match (&self.main_ctx, self.main_coro) {
            (Some(m), Some(mc)) => {
                let ci = match count {
                    CiCount::Array51 => m.frame_size,
                    // the levels below its running one, as for the running
                    // thread ([`Vm::ci_in_use`])
                    CiCount::InUse => {
                        let levels = (m.frames.len() + m.natives.len()) as u32 + self.g.host_frames;
                        levels - m.meta_conts - m.host_conts
                    }
                    CiCount::Allocated => m.ci_alloc,
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
            ci: match count {
                CiCount::Array51 => self.g.frame_size,
                CiCount::InUse => self.ci_in_use(),
                CiCount::Allocated => self.g.ci_alloc,
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
    pub(in super::super) fn gc_propagate(
        &mut self,
        work: &mut i64,
        floor: i64,
        mut run: Running,
    ) -> bool {
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
    pub(in super::super) fn collector_shrinks_thread(&mut self, co: Gc<crate::runtime::Coro>) {
        let running = self.current.or(self.main_coro);
        if running.is_some_and(|c| c.ptr_eq(co)) {
            let inuse = self.stack_in_use();
            self.shrink_running(super::stack_inuse::Shrink::Collector, inuse);
        } else if self.main_ctx.is_some() && self.main_coro.is_some_and(|c| c.ptr_eq(co)) {
            self.collector_shrinks_main(crate::vm::exec::stack_inuse::Shrink::Collector);
        } else {
            self.collector_shrinks_coro(co, crate::vm::exec::stack_inuse::Shrink::Collector);
        }
    }

    /// The list sweep of a state that sweeps `quota` objects: how many
    /// it swept and the bytes it freed; `None` once the list is done.
    pub(in super::super) fn sweep_quota(&mut self, quota: i64) -> Option<(i64, i64)> {
        if !self.heap.gc_phase_is_sweep() {
            return None;
        }
        let before = self.heap.bytes() as i64;
        let (n, _) = self.heap.sweep_counted(quota);
        Some((n, before - self.heap.bytes() as i64))
    }
}
