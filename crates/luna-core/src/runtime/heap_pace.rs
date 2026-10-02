//! Byte accounting and collection pacing.

use super::*;

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
    /// The walk visits both the live `all` list and the
    /// `sweep_cur` detached list so a mid-cycle invocation reports
    /// the same total as [`Heap::live_objects`].
    pub fn walk_objects(&self, mut visit: impl FnMut(ObjTag)) {
        for head in [self.all, self.sweep_cur] {
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
        self.bytes >= self.gc_limit
    }

    /// `collectgarbage("stop"/"restart")`: suspend or resume auto-GC.
    pub(crate) fn gc_is_stopped(&self) -> bool {
        self.gc_stopped
    }

    pub(crate) fn gc_set_stopped(&mut self, stopped: bool) {
        self.gc_stopped = stopped;
        self.set_next_gc(self.next_gc);
    }

    pub(super) fn set_next_gc(&mut self, next: usize) {
        self.next_gc = next;
        self.gc_limit = if self.gc_stopped { usize::MAX } else { next };
    }

    /// Re-arm with caller-supplied `pause` (PUC param, % of live bytes). The
    /// next cycle fires once `bytes >= live * pause / 100`. `pause=200` (PUC
    /// default) waits for the heap to double; `pause=100` fires immediately
    /// when alloc resumes; `pause=300` is 3× — lower pause = more aggressive.
    pub fn rearm_gc_pause(&mut self, pause: i64) {
        let pause = pause.max(0) as usize;
        let target = self
            .bytes
            .saturating_mul(pause)
            .saturating_div(100)
            .max(GC_MIN_THRESHOLD);
        self.set_next_gc(target);
    }

    /// Re-arm the auto-GC threshold after a collection (PUC pause-style: next
    /// collection once the live set roughly doubles).
    pub fn rearm_gc(&mut self) {
        self.set_next_gc(self.bytes.saturating_mul(2).max(GC_MIN_THRESHOLD));
    }

    /// Apply a `before → after` box-size delta from a Table mutation
    /// (`set`/`rehash`/`ensure_*`). Grows credit `Heap.bytes`; shrinks
    /// debit it. `free_obj` for `ObjTag::Table` then subtracts the table's
    /// final `internal_bytes()` so the round-trip is symmetric across the
    /// table's whole lifetime.
    pub(crate) fn apply_bytes_delta(&mut self, before: usize, after: usize) {
        if after > before {
            self.bytes += after - before;
        } else if before > after {
            self.bytes = self.bytes.saturating_sub(before - after);
        }
    }
}
