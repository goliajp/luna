//! The incremental cycle: propagating, the atomic step, and the work
//! PUC's collector counts for each.

use super::*;

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

    /// Conclude a Propagate cycle: drain any residual gray, run the atomic
    /// tail (weak / ephemeron / finalizer / proto-cache / flip), detach `all`
    /// and enter `GcPhase::Sweep`. Releases `propagate`.
    /// PUC `atomic` + `entersweep` transition.
    pub(crate) fn gc_finish_atomic(&mut self) {
        self.gc_finish_atomic_work(None);
    }

    /// [`Heap::gc_finish_atomic`], counting the work PUC's `atomic` counts
    /// for the objects it traverses (with `run` the running thread), and
    /// entering the sweep as the dialect does: 5.1 sweeps nothing yet, 5.3
    /// one object, the others to the first object kept. Returns the work
    /// and how many objects that sweep went over.
    pub(crate) fn gc_finish_atomic_work(&mut self, run: Option<Running>) -> (i64, i64) {
        debug_assert!(self.phase == GcPhase::Propagate);
        let mut m = self.loan_marker();
        // any residual gray (caller may not have drained to empty), then
        // what the back barriers sent to gray again (PUC `grayagain`)
        let mut work = self.drain_work(&mut m, run);
        self.regray_again(&mut m);
        work += self.drain_work(&mut m, run);
        m.drain_quiet();
        // a thread stays gray while marking (`grayagain`): `atomic`
        // traverses each one marked again
        if let Some(run) = run {
            let mut seen_run = false;
            for i in 0..self.threads.len() {
                let co = self.threads[i];
                let h = co.as_ptr() as *mut GcHeader;
                seen_run |= h == run.h;
                // SAFETY: a listed thread is allocated until the sweep
                unsafe {
                    if !is_white((*h).flags) {
                        let w = self.object_work(h, run);
                        #[cfg(feature = "gc-trace")]
                        self.trace_prop(h, w, run);
                        work += w;
                    }
                }
            }
            // the main thread, luna's own object, is on no list
            if !seen_run && !run.h.is_null() {
                // SAFETY: the running thread is allocated
                // SAFETY: as above
                let w = unsafe { self.object_work(run.h, run) };
                #[cfg(feature = "gc-trace")]
                // SAFETY: as above
                unsafe {
                    self.trace_prop(run.h, w, run)
                };
                work += w;
            }
        }
        // pre-atomic ephemeron convergence
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: as in `mark_all`: `t` is a marked table from `m.ephemeron`, nothing is freed before the sweep, and the marker holds no other reference to it
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, &mut m) };
                }
                drain_marker(&mut m);
                if !changed {
                    break;
                }
            }
        }
        self.atomic_tail(&mut m);
        if self.table_dialect == crate::runtime::table::Dialect::L52 {
            self.color_fixed_strings(0);
        }
        self.settle_sizer(&mut m);
        #[cfg(feature = "gc-trace")]
        self.trace_listed();
        if run.is_some() {
            self.collect_dead_strings();
        } else {
            self.end_string_sweep();
        }
        // PropagateState consumed; the sweep goes over `all` in place
        // (PUC `entersweep`). Anything allocated past this point goes in
        // front of where it is and survives.
        self.propagate = None;
        debug_assert!(
            self.gray.is_empty() && !self.gray_overflow,
            "gray queue not drained at atomic"
        );
        // the newest object at the last safe point may be freed by the
        // sweep: until the next safe point every object counts as young
        self.young = ptr::null_mut();
        self.phase = GcPhase::Sweep;
        self.sweep_prev = ptr::null_mut();
        let swept = match self.table_dialect {
            crate::runtime::table::Dialect::L51 => 0,
            crate::runtime::table::Dialect::L53 => self.sweep_objects(1, false) as i64,
            _ => self.sweep_objects(usize::MAX, true) as i64,
        };
        (work, swept)
    }

    /// Drain `m`, with the work PUC counts for each object when `run`
    /// names the running thread.
    fn drain_work(&mut self, m: &mut Marker, run: Option<Running>) -> i64 {
        let Some(run) = run else {
            drain_marker(m);
            return 0;
        };
        let mut work = 0;
        while let Some(h) = m.pop() {
            m.leaf_bytes = 0;
            // SAFETY: as in `propagate_work`
            unsafe {
                blacken(h, m);
                let w = self.object_work(h, run) + m.leaf_bytes;
                #[cfg(feature = "gc-trace")]
                self.trace_prop(h, w, run);
                work += w;
            }
        }
        work
    }

    /// The objects a back barrier sent to gray, on `m`'s stack to be
    /// traversed again.
    pub(super) fn regray_again(&mut self, m: &mut Marker) {
        while let Some(h) = self.grayagain.pop() {
            m.push_gray(h);
        }
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
/// counts), which the VM holds while it runs; and the main thread's
/// object, stack size and `CallInfo`s while a coroutine runs (null else),
/// which the VM holds too.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Running {
    pub(crate) h: *mut GcHeader,
    pub(crate) top: u32,
    pub(crate) size: u32,
    pub(crate) ci: u32,
    pub(crate) main: *mut GcHeader,
    pub(crate) main_size: u32,
    pub(crate) main_ci: u32,
}

/// What a 5.5 marking needs to count the objects it marks (`GCmarked`):
/// the sizes, luna's own tables (lent by the heap meanwhile) and the
/// threads the VM holds.
pub(crate) struct Sizer {
    sizes: crate::runtime::puc_size::PucSizes,
    own: Vec<*const Table>,
    stand_ins: Vec<(*mut GcHeader, usize)>,
    run: Running,
    pub(in crate::runtime::heap) marked: i64,
}

impl Sizer {
    /// 5.5's `objsize` of the object behind `h`, which marking it adds.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    pub(in crate::runtime::heap) unsafe fn size(&self, h: *mut GcHeader) -> i64 {
        let d = crate::runtime::table::Dialect::L55;
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            if (*h).tag != ObjTag::Coro {
                if is_uncounted(h) {
                    // what an object of luna's stands for
                    let s = self.stand_ins.iter().filter(|s| s.0 == h);
                    return s.map(|s| s.1).sum::<usize>() as i64;
                }
                return pace::objsize(&self.sizes, d, &self.own, h) as i64;
            }
            let r = &self.run;
            let (size, ci) = if h == r.h {
                (r.size, r.ci)
            } else if h == r.main {
                (r.main_size, r.main_ci)
            } else {
                let c = &*(h as *const crate::runtime::Coro);
                (c.stack_size, c.ci_alloc)
            };
            pace::thread_bytes(&self.sizes, d, size, ci) as i64
        }
    }
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
            if self.table_dialect == crate::runtime::table::Dialect::L53
                && unsafe { (*h).tag } == ObjTag::Coro
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

impl Heap {
    /// A sizer for a marker, when markings count `GCmarked`.
    pub(in crate::runtime::heap) fn lend_sizer(&mut self) -> Option<Sizer> {
        let run = self.pace.run.filter(|_| self.pace.count_marked)?;
        Some(Sizer {
            sizes: self.sizes,
            own: std::mem::take(&mut self.puc.own_tables),
            stand_ins: std::mem::take(&mut self.puc.stand_ins),
            run,
            marked: 0,
        })
    }

    /// The sizer `m` had back: what it counted, and the tables it held.
    pub(in crate::runtime::heap) fn settle_sizer(&mut self, m: &mut Marker) {
        if let Some(s) = m.sizer.take() {
            self.pace.estimate += s.marked;
            self.puc.own_tables = s.own;
            self.puc.stand_ins = s.stand_ins;
        }
    }

    /// The object behind `h` marked outside a marker (a barrier): 5.5
    /// counts it.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(in crate::runtime::heap) unsafe fn count_marked(&mut self, h: *mut GcHeader) {
        if let Some(s) = self.lend_sizer() {
            // SAFETY: the caller's contract
            let n = unsafe { s.size(h) };
            self.pace.estimate += n;
            self.puc.own_tables = s.own;
            self.puc.stand_ins = s.stand_ins;
        }
    }
}
