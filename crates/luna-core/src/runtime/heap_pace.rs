//! Byte accounting and collection pacing.

use super::*;
use crate::runtime::table::Dialect;

#[path = "gc_work.rs"]
mod work;
pub(super) use work::{array_bytes, hash_bytes, native_upvals, objsize, stack_bytes, thread_bytes};

impl Heap {
    /// Number of GC-managed objects currently linked into the heap (live + not
    /// yet swept). Useful for `collectgarbage("count")`-style introspection.
    pub fn live_objects(&self) -> usize {
        self.live
    }

    /// Approximate heap size in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// What a memory cap is held against: the bytes the allocation context
    /// has handed out, when it counts them (a host function or a policy),
    /// else PUC's count ([`Heap::bytes`]).
    pub fn cap_bytes(&self) -> usize {
        self.mem_ctx().in_use().unwrap_or(self.bytes)
    }

    /// Pure-read walk over the intrusive `all`
    /// objects list, invoking `visit(tag)` once per live (or
    /// not-yet-swept) GC-managed object. Used by `luna-tools`'s
    /// `luna-heap-dump` to build a per-type histogram; embedders
    /// can reuse it for ad-hoc heap introspection.
    ///
    /// # Read-only contract
    ///
    /// The callback receives only the [`ObjTag`] discriminant and
    /// is invoked under a `&self` borrow on the heap: no pointer
    /// to the GC payload escapes, no `as_mut`-style aliasing is
    /// available, and the walk performs zero allocation in the
    /// loop. Safe to call between dispatch ticks (the only allocs
    /// happen in the caller's bookkeeping).
    ///
    /// The walk visits the `all` list (in a sweep, with the dead objects
    /// not yet swept) and the never-swept fixed list, so a mid-cycle
    /// invocation reports the same total as [`Heap::live_objects`].
    pub fn walk_objects(&self, mut visit: impl FnMut(ObjTag)) {
        for head in [self.all, self.fixed] {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: pointers come from the runtime's
                // intrusive all-objects list. `&self` borrow on
                // the heap prevents concurrent mutation; the GC
                // cannot run while this walk holds the borrow,
                // so every `next` link is valid until consumed.
                let (tag, next) = unsafe { ((*cur).tag, (*cur).next) };
                visit(tag);
                cur = next;
            }
        }
    }

    /// Whether allocation has crossed the auto-GC threshold (cheap safe-point
    /// check for the interpreter loop).
    #[inline(always)]
    pub fn gc_due(&self) -> bool {
        self.gc_due_after(0)
    }

    /// [`Heap::gc_due`] once `more` bytes more are counted.
    pub(crate) fn gc_due_after(&self, more: usize) -> bool {
        self.bytes + more >= self.gc_limit && !self.no_gc() && !self.no_auto()
    }

    /// No automatic step runs (`gc-trace` with `R38_NOAUTO` set).
    #[inline(always)]
    pub(crate) fn no_auto(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.trace.no_auto;
        #[cfg(not(feature = "gc-trace"))]
        false
    }

    /// No collection runs (`gc-trace` with `R38_NOGC` set).
    #[inline(always)]
    pub(crate) fn no_gc(&self) -> bool {
        #[cfg(feature = "gc-trace")]
        return self.trace.no_gc;
        #[cfg(not(feature = "gc-trace"))]
        false
    }

    /// `collectgarbage("stop"/"restart")`: suspend or resume auto-GC.
    pub(crate) fn gc_is_stopped(&self) -> bool {
        self.gc_stopped
    }

    /// `collectgarbage("stop")` / `"restart"`. A restart makes a step due
    /// at once (5.1 `GCthreshold = totalbytes`, later `luaE_setdebt(g, 0)`).
    pub(crate) fn gc_set_stopped(&mut self, stopped: bool) {
        self.gc_stopped = stopped;
        if !stopped {
            let d = self.table_dialect;
            match d {
                Dialect::L51 => self.pace.set_threshold(self.bytes as i64),
                _ => self.pace.set_debt(d, self.bytes, 0),
            }
        }
        self.refresh_gc_limit();
    }

    /// 5.1: an explicit step sets the threshold that `stop` had put out of
    /// reach, so the collector runs on its own again after it.
    pub(crate) fn gc_unstop_51(&mut self) {
        self.gc_stopped = false;
    }

    /// The stop flag put back as it was, with no debt set (PUC `GCTM`
    /// restoring `gcrunning`).
    pub(crate) fn gc_restore_stopped(&mut self, stopped: bool) {
        self.gc_stopped = stopped;
        self.refresh_gc_limit();
    }

    /// Whether PUC's `luaC_step` runs a step (`gcrunning`): not while the
    /// collector is stopped, nor while a 5.4 or 5.5 finalizer runs.
    pub(crate) fn gc_steps_run(&self) -> bool {
        !self.gc_stopped && !self.gc_fin_stop
    }

    /// The count at which the next step falls due, from the pace. 5.1
    /// stops the collector with an unreachable threshold; the later
    /// dialects still step then, to put the debt back.
    pub(crate) fn refresh_gc_limit(&mut self) {
        self.gc_limit = if self.gc_stopped && self.table_dialect == Dialect::L51 {
            usize::MAX
        } else {
            self.pace.limit(self.table_dialect)
        };
    }

    /// A cycle ended with the count at what it is now: the pause before
    /// the next one, `pause` in percent of what survived (PUC `setpause`;
    /// 5.1 `setthreshold`).
    pub fn rearm_gc_pause(&mut self, pause: i64) {
        use crate::runtime::gc_pace::*;
        let d = self.table_dialect;
        let bytes = self.bytes;
        match d {
            Dialect::L51 => {
                self.pace.estimate = bytes as i64;
                self.pace.set_threshold(threshold_51(bytes as i64, pause));
            }
            Dialect::L55 => {
                let marked = bytes as i64 - self.fixed_bytes();
                self.pace.estimate = marked;
                let debt = pause_debt_55(bytes, marked.saturating_mul(pause) / 100);
                self.pace.set_debt(d, bytes, debt);
            }
            _ => {
                self.pace.estimate = bytes as i64;
                let debt = pause_debt(d, bytes, bytes as i64, pause);
                self.pace.set_debt(d, bytes, debt);
            }
        }
        self.pace.state = St::Pause;
        self.refresh_gc_limit();
    }

    /// [`Heap::rearm_gc_pause`] with PUC's usual pause (200%).
    pub fn rearm_gc(&mut self) {
        self.rearm_gc_pause(200);
    }

    /// The bytes of PUC's objects that no collection marks (`fixedgc`:
    /// the reserved words, the metamethod names, `_ENV`, the memory
    /// error's message).
    pub(crate) fn fixed_bytes(&self) -> i64 {
        let s = self.sizes;
        let mut n: i64 = self
            .puc
            .pins
            .iter()
            .map(|p| s.string(p.len(), false) as i64)
            .sum();
        n += s.string(b"not enough memory".len(), false) as i64;
        n
    }

    /// Every live object is rooted now (an interpreter safe point, or the
    /// end of a collection): objects allocated from here on are young
    /// until the next such point.
    #[inline(always)]
    pub(crate) fn safe_point(&mut self) {
        self.young = self.all;
    }

    /// Mark every object allocated since the last [`Heap::safe_point`],
    /// and the object that was newest then, which keeps the boundary from
    /// being freed (and its address reused) by the collection. An
    /// emergency collection runs in the middle of an operation, whose new
    /// objects may be held only by Rust locals.
    pub(crate) fn mark_young(&self, m: &mut Marker) {
        let mut cur = self.all;
        while !cur.is_null() {
            // SAFETY: `cur` is on `all`, whose objects are allocated (nothing
            // is freed while marking); only the header is touched
            unsafe {
                m.header(cur);
                if cur == self.young {
                    break;
                }
                cur = (*cur).next;
            }
        }
    }
}
