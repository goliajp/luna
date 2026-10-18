//! What the steps of every dialect share: the threads the collector
//! finds, the propagation, the atomic step and the sweep's quota.

use super::*;

impl Vm {
    /// The running thread, whose stack the VM holds.
    pub(super) fn gc_running(&self) -> Running {
        let co = self.current.or(self.main_coro);
        let main = match (&self.main_ctx, self.main_coro) {
            (Some(m), Some(mc)) => {
                let ci = match self.heap.table_dialect {
                    Dialect::L51 => m.frame_size,
                    // the levels below its running one, as for the running
                    // thread ([`Vm::ci_in_use`])
                    Dialect::L52 => {
                        let levels = (m.frames.len() + m.natives.len()) as u32 + self.g.host_frames;
                        levels - m.meta_conts - self.g.host_pcalls
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
            top: self.gc_top,
            size: self.stack_size,
            // 5.1 counts its `CallInfo` array, 5.2 the ones in use below
            // the running one, the later dialects all it has
            ci: match self.heap.table_dialect {
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

    /// The threads as the collector is about to find them.
    pub(crate) fn gc_hold_threads(&mut self) {
        self.heap.pace.run = Some(self.gc_running());
    }

    /// A marking starts from the roots: 5.5's `GCmarked` counts from 0
    /// what it marks (`restartcollection`).
    pub(crate) fn gc_count_marked(&mut self) {
        let count = self.heap.table_dialect == Dialect::L55;
        self.heap.pace.count_marked = count;
        if count {
            self.heap.pace.estimate = 0;
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
            self.collector_shrinks_main();
        } else {
            // SAFETY: `h`, gray, heads a live coroutine
            let co = unsafe { crate::runtime::heap::Gc::from_ptr(h as *mut _) };
            self.collector_shrinks_coro(co);
        }
    }

    /// `restartcollection` (5.1 `markroot`): the roots marked, the cycle
    /// propagating.
    pub(super) fn gc_restart(&mut self) {
        self.heap.puc.shrunk.clear();
        self.gc_count_marked();
        let mut m = self.heap.gc_begin_propagate();
        self.mark_roots_restart(&mut m);
        // 5.1 marks them in its atomic step
        if self.version != LuaVersion::Lua51 {
            self.heap.mark_tobefnz(&mut m);
        }
        self.heap.stash_marker(m);
        self.heap.pace.state = St::Propagate;
    }

    /// PUC `atomic` and `entersweep`; returns the work and how many
    /// objects the sweep went over on the way in.
    pub(super) fn gc_atomic(&mut self) -> (i64, i64) {
        self.clear_dead_stack();
        let mut run = self.gc_running();
        // 5.2's final traversal of a thread counts no `CallInfo`
        if self.heap.table_dialect == Dialect::L52 {
            run.ci = 0;
            run.main_ci = 0;
        }
        let mut m = self.heap.loan_marker();
        if self.version == LuaVersion::Lua51 {
            self.heap.mark_tobefnz(&mut m);
        }
        // 5.3 counts what the roots other than the threads reach; the
        // threads, gray again, are traversed with `grayagain`, uncounted
        let roots = if self.version == LuaVersion::Lua53 {
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
        let (work, swept) = self.heap.gc_finish_atomic_work(Some(run));
        let r = (work + roots, swept);
        // 5.3 shrank them as it traversed them
        if self.version != LuaVersion::Lua53 {
            self.collector_shrinks_threads();
        }
        self.forget_dead_errors();
        // there may be objects to be finalized
        self.heap.pace.fin_num = 1;
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

/// A sweep over one of PUC's other lists (the objects with finalizers,
/// the ones to be finalized), which luna keeps on its one list: `left`
/// goes down by `quota`; `None` once it is done.
pub(super) fn sweep_other(left: &mut Option<i64>, quota: i64) -> bool {
    match left {
        Some(n) => {
            *n -= quota;
            if *n <= 0 {
                *left = None;
            }
            true
        }
        None => false,
    }
}
