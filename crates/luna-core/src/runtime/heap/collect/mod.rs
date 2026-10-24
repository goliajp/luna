//! Full collections and the incremental propagate / atomic phases.

use super::*;

mod step;
pub(super) use step::Sizer;
pub(crate) use step::{Propagated, Running};

impl Heap {
    /// Mark from `roots`, sweep everything unreachable. Returns the number of
    /// objects freed.
    pub fn collect(&mut self, roots: &[Value]) -> usize {
        self.collect_ex(roots, &[])
    }

    /// Like `collect`, with additional bare-object roots (e.g. the VM's open
    /// upvalues, which are not first-class Values).
    pub(crate) fn collect_ex(&mut self, roots: &[Value], extra: &[Gc<Upvalue>]) -> usize {
        let seed = |m: &mut Marker| {
            for &r in roots {
                m.value(r);
            }
            for &uv in extra {
                m.mark(uv);
            }
        };
        if self.phase == GcPhase::Propagate {
            let mut m = self.loan_marker();
            seed(&mut m);
            self.stash_marker(m);
            self.gc_finish_atomic();
        }
        self.gc_finish_sweep();
        let mut m = self.gc_begin_full();
        seed(&mut m);
        self.gc_end_full(m)
    }

    /// Finish an incremental sweep in progress, so the whole heap is on
    /// `all` again with every mark cleared.
    pub(crate) fn gc_finish_sweep(&mut self) {
        self.end_string_sweep();
        if self.phase == GcPhase::Sweep {
            self.gc_sweep_step(usize::MAX);
        }
    }

    /// Start a full stop-the-world collection (PUC `fullinc`): the
    /// marker the caller marks its roots with, then hands to
    /// [`Heap::gc_end_full`]. Any in-flight incremental cycle must have been
    /// finished first (`gc_finish_atomic`, `gc_finish_sweep`). Barrier-
    /// grayed objects carried over stay queued, and the gray stack's block
    /// goes back to `gray` afterwards, so a collection does not regrow it.
    pub(crate) fn gc_begin_full(&mut self) -> Marker {
        debug_assert!(self.phase == GcPhase::Pause);
        self.grayagain = mark::GrayStack::default();
        self.puc_mark_begins();
        let (sizer, lent) = self.puc_lend(false);
        Marker {
            stack: self.gray.take(),
            overflow: std::mem::take(&mut self.gray_overflow),
            scan: self.all,
            weak: Vec::new(),
            ephemeron: Vec::new(),
            no_ephemeron: !self.rules.ephemeron(),
            rules: *self.puc_rules(),
            cached_protos: Vec::new(),
            leaf_black: LEAF,
            leaf_hdr: 0,
            leaf_bytes: 0,
            leaf_upval: 0,
            sizer,
            quiet: Vec::new(),
            quiet_mode: false,
            lent,
            verify: Default::default(),
        }
    }

    /// Finish the collection [`Heap::gc_begin_full`] started, with the
    /// roots marked in `m`: mark from them, run the atomic tail and sweep.
    /// Returns the number of objects freed.
    pub(crate) fn gc_end_full(&mut self, mut m: Marker) -> usize {
        self.mark_from(&mut m);
        self.gc_sweep_full()
    }

    /// The sweep of [`Heap::gc_end_full`], after its marking.
    pub(crate) fn gc_sweep_full(&mut self) -> usize {
        self.puc_full_sweep_begins();
        let n = self.full_sweep();
        self.end_string_sweep();
        n
    }

    /// Stop-the-world mark from the roots already in `m`: tobefnz, drain,
    /// then the atomic tail (weak / ephemeron / finalizer resurrection /
    /// current-white flip). After return all reachable objects are BLACK
    /// and `current_white` has flipped, so the caller's sweep tests
    /// `other-white` for dead. Does NOT change `phase`.
    pub(crate) fn mark_from(&mut self, m: &mut Marker) {
        // objects already queued for finalization but not yet run must stay
        // alive until the VM calls their `__gc` (they may be unreachable now).
        for &h in self.tobefnz_live() {
            // SAFETY: a queued finalizable stays allocated until its `__gc` has run (`take_tobefnz`)
            unsafe { m.header(h) };
        }
        drain_marker(m);
        // ephemeron convergence: a weak-key entry's value is reachable only if
        // the key is. Marking a value can make another key reachable, so repeat
        // until no value is newly marked (PUC convergeephemerons).
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: `t` is a table `Table::trace` pushed onto `m.ephemeron` this cycle, so it is marked; nothing is freed until the sweep after this mark, and the marker holds no other reference to the table
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, m) };
                }
                drain_marker(m);
                if !changed {
                    break;
                }
            }
        }
        self.atomic_tail(m, None);
        self.settle_sizer(m);
        self.hook_listed();
        debug_assert!(m.stack.is_empty() && !m.overflow);
        self.gray = m.stack.take();
    }

    /// PUC `atomic()` tail: weak-table value-clear, finalizer resurrection,
    /// post-resurrection ephemeron convergence, proto cache, key-clear, late
    /// value-clear, and current-white flip. Marker is consumed; `weak` is
    /// empty on return.
    ///
    /// Shared between the STW path (`mark_all`) and the incremental path
    /// (`gc_finish_atomic`). PUC 5.5 `lgc.c::atomic` mirror:
    ///   propagate → remarkupvals → convergeephemerons
    ///   → clearbyvalues(weak, NULL)            ─ early value-clear
    ///   → clearbyvalues(allweak, NULL)         ─ (same pass under luna)
    ///   → origweak = g->weak                   ─ snapshot pre-resurrection
    ///   → separatetobefnz(0) + markbeingfnz    ─ separate_finalizables
    ///   → propagateall + convergeephemerons    ─ post-resurrection
    ///   → clearbykeys(ephemeron) + clearbykeys(allweak)
    ///   → clearbyvalues(weak, origweak)        ─ NEW (post-resurrect) only
    ///   → clearbyvalues(allweak, origall)      ─ (same)
    /// The `origweak` split matters because finalizer resurrection can
    /// re-trace fresh proto/closure → new weak tables joining `m.weak`;
    /// PUC limits the late value-clear to those new heads.
    pub(super) fn atomic_tail(&mut self, m: &mut Marker, run: Option<Running>) -> i64 {
        let early_is_dead = |v: Value| -> bool {
            let h = match v {
                Value::Str(_) => return false,
                Value::Table(t) => t.as_ptr() as *mut GcHeader,
                Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                Value::Native(n) => n.as_ptr() as *mut GcHeader,
                Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                _ => return false,
            };
            // SAFETY: `h` is the object of a value stored in a weak table being cleared; the sweep that frees unmarked objects has not run yet, so the header is still allocated (it is only read)
            unsafe { is_white((*h).flags) }
        };
        let mark_string = |v: Value| {
            if let Value::Str(s) = v {
                // SAFETY: `s` is a string stored in a weak table being cleared; it is not freed before the sweep that follows, and only its flag byte is written
                unsafe {
                    let h = s.as_ptr() as *mut GcHeader;
                    // strings are leaves: skip gray and go straight to black
                    (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | BLACK);
                }
            }
        };
        // (1) early clearbyvalues — drop dead-value entries from every weak
        // table on `m.weak` (PUC's combined `clearbyvalues(weak, NULL) +
        // clearbyvalues(allweak, NULL)`). Keys are deferred to the
        // post-resurrection sweep below.
        for t in &m.weak {
            // SAFETY: `t` was pushed onto `m.weak` by `Table::trace` this cycle, so it is marked and stays allocated through this cycle's sweep; no other reference to the table is live while it is cleared
            unsafe {
                let (_wk, wv) = (**t).weak_mode();
                if wv {
                    (**t).clear_weak(false, true, &early_is_dead, &mark_string);
                }
            }
        }
        // (2) `origweak` snapshot: the length of `m.weak` before resurrection;
        // a table past it is a "NEW" weak table the resurrection brought in
        let origweak_n = m.weak.len();
        // (3) separate + markbeingfnz — resurrect every registered finalizable
        // that ended up unmarked. `m.header(h)` enqueues each into the marker
        // so the following drain_marker propagates through it.
        let mut dead = self.separate_finalizables(m);
        // 5.1 counts what they keep alive with them (`propagateall`'s work)
        dead += if self.puc_rules().counts_finalizable_work {
            self.drain_work(m, run)
        } else {
            drain_marker(m);
            0
        };
        // (4) post-resurrection ephemeron convergence — a resurrected
        // finalizable may bring new keys into reach, which in turn marks new
        // ephemeron values.
        if !m.ephemeron.is_empty() {
            loop {
                let mut changed = false;
                let eph = m.ephemeron.clone();
                for t in eph {
                    // SAFETY: as in `mark_all`: `t` is a marked table from `m.ephemeron`, nothing is freed before the sweep, and the marker holds no other reference to it
                    changed |= unsafe { (*t).converge_ephemeron(&weak_key_alive, m) };
                }
                drain_marker(m);
                if !changed {
                    break;
                }
            }
        }
        // (5) closure-cache weak refs (PUC `traverseproto`): a closure kept
        // only by a proto's cache is dead, or its upvalues' `__gc` never run
        for &p in &m.cached_protos {
            // SAFETY: `p` was pushed onto `m.cached_protos` by `Proto::trace` this cycle (it is marked); its cached closure, white or not, is not freed before this cycle's sweep
            unsafe {
                if let Some(c) = (*p).cache.get() {
                    let h = c.as_ptr() as *mut GcHeader;
                    if is_white((*h).flags) {
                        (*p).cache.set(None);
                    }
                }
            }
        }
        // the VM's watched values and the chunk roots are weak: dead ones
        // are noted or forgotten before they are freed
        for w in &mut self.weak_watch {
            w.1 = !survived_mark(w.0);
        }
        for w in &mut self.weak_objs {
            // SAFETY: a watched object is allocated until this cycle's
            // sweep, which has not run; only its flag byte is read
            w.1 = unsafe { is_white((*w.0).flags) };
        }
        self.chunk_roots.retain(|p| {
            // SAFETY: a root still listed was not freed (dead ones leave the
            // list first) and this cycle's sweep has not run
            !unsafe { is_white((*(p.as_ptr() as *mut GcHeader)).flags) }
        });
        // (6) clearbykeys — drop entries whose weak key did not survive
        // marking, across every weak table (PUC's `clearbykeys(ephemeron)
        // + clearbykeys(allweak)`). Pure key sweep — value-dead entries are
        // either already nil from step (1) or wait for step (7).
        for t in &m.weak {
            // SAFETY: `t` is a marked weak table from `m.weak` (see step 1); nothing has been freed since it was pushed
            unsafe {
                let (wk, _wv) = (**t).weak_mode();
                if wk {
                    (**t).clear_weak(true, false, &early_is_dead, &mark_string);
                }
            }
        }
        // (7) late clearbyvalues — PUC's `clearbyvalues(weak, origweak) +
        // clearbyvalues(allweak, origall)`. Limit the sweep to NEW heads so
        // we don't redo work already done in step (1) for the pre-resurrect
        // tables (they were drained by then and re-marking happens through
        // mark_string in step (6)). resurrected weak tables joining `m.weak`
        // past `origweak_n` get their first value-clear here.
        let weak_snapshot = std::mem::take(&mut m.weak);
        for t in &weak_snapshot[origweak_n..] {
            // SAFETY: `t` is a marked weak table from `m.weak` (see step 1); nothing has been freed since it was pushed
            unsafe {
                let (_wk, wv) = (**t).weak_mode();
                if wv {
                    (**t).clear_weak(false, true, &early_is_dead, &mark_string);
                }
            }
        }
        // PUC 5.5 `atomic` end: flip currentwhite so survivors (presently
        // BLACK) get transitioned into the *new* current-white during sweep,
        // and the pre-flip current-white becomes the dead-white (the bit the
        // sweep tests for). Born-during-sweep allocations stamp the new
        // current-white via `Heap::link`, so they survive this cycle.
        // PUC `luaS_clearcache`, after every weak table's clearing
        self.puc_clear_string_cache();
        self.doom_open_upvals();
        self.current_white ^= WHITE_BITS;
        // the threads the sweep is about to free are no longer visited
        self.threads.retain(|&c| header_is_marked(c));
        // `gc-verify` — tricolor invariant check at the one
        // moment it is exact: marking is complete, nothing is freed yet.
        // A BLACK (surviving) table holding a dead-white child means a
        // write barrier was missed; the child will be freed by the
        // upcoming sweep and the table left holding a dangling pointer.
        // Nothing is dangling *yet*, so the reporter may safely print
        // string contents to identify the key.
        self.check_after_atomic("atomic_tail");
        dead
    }

    /// Borrow Heap's persistent propagate state as an ephemeral Marker.
    /// Caller MUST call `stash_marker` with the same Marker after work to
    /// write the (potentially mutated) state back. Used by the incremental
    /// Propagate path to avoid lifetime entanglement between `&mut self` and
    /// `&mut self.propagate`.
    pub(crate) fn loan_marker(&mut self) -> Marker {
        let mut prop = self
            .propagate
            .take()
            .expect("propagate state taken outside Propagate phase");
        let (sizer, lent) = self.puc_lend(true);
        Marker {
            stack: self.gray.take(),
            overflow: std::mem::take(&mut self.gray_overflow),
            scan: self.all,
            weak: std::mem::take(&mut prop.weak),
            ephemeron: std::mem::take(&mut prop.ephemeron),
            no_ephemeron: prop.no_ephemeron,
            rules: *self.puc_rules(),
            cached_protos: std::mem::take(&mut prop.cached_protos),
            leaf_black: 0,
            leaf_hdr: self.puc_rules().leaf_hdr,
            leaf_bytes: 0,
            leaf_upval: self.puc_rules().leaf_upval,
            sizer,
            quiet: std::mem::take(&mut prop.quiet),
            quiet_mode: false,
            lent,
            verify: Default::default(),
        }
    }

    pub(crate) fn stash_marker(&mut self, mut m: Marker) {
        self.settle_sizer(&mut m);
        let no_ephemeron = m.no_ephemeron;
        self.gray = m.stack;
        self.gray_overflow = m.overflow;
        self.propagate = Some(PropagateState {
            quiet: m.quiet,
            weak: m.weak,
            ephemeron: m.ephemeron,
            cached_protos: m.cached_protos,
            no_ephemeron,
        });
    }
}
