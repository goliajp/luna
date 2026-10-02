//! Finalizer bookkeeping: registering objects with `__gc` and queueing them.

use super::*;

impl Heap {
    /// Move every registered finalizable that is now unreachable to `tobefnz`
    /// and resurrect it (mark it via `m`) so it — and the data its `__gc` needs
    /// — survives this cycle. Survivors stay registered in `finalize`. PUC's
    /// `separatetobefnz` walks `g->finobj` head-first, but `g->finobj` is a
    /// linked list that registration *prepends* to — so dead objects end up
    /// in `tobefnz` in reverse registration order, and `__gc` ultimately
    /// runs LIFO. luna's `finalize` is a Vec that grows forward, so iterate
    /// it in reverse here to match the LIFO contract (gc.lua's userdata
    /// section asserts the finalizers fire from value 10 back to 0).
    pub(super) fn separate_finalizables(&mut self, m: &mut Marker) {
        let mut i = self.finalize.len();
        while i > 0 {
            i -= 1;
            let h = self.finalize[i];
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            if unsafe { is_white((*h).flags) } {
                // Two-pass cycle-finalize (PUC 5.3 gc.lua :502): when a
                // finalizable table holds onto an unreachable coroutine, the
                // cycle (table → coroutine.stack → closure → table) keeps the
                // mark phase from reaching the table even though it is still
                // logically alive for one more GC pass. PUC's mark-sweep wakes
                // it via `markbeingfnz` *after* sweeping, so the actual `__gc`
                // call lands one cycle later. luna mirrors this by resurrecting
                // the table on the first sighting and only enqueuing it for
                // `__gc` on the second.
                let in_thread_cycle = self.defer_thread_cycle_finalize
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    && unsafe { (*h).tag } == ObjTag::Table
                    && {
                        let t = h as *mut Table;
                        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                        unsafe { (*t).refs_contain_unmarked_coro() }
                    };
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                let already_deferred = unsafe { (*h).flags & DEFERRED != 0 };
                if in_thread_cycle && !already_deferred {
                    // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                    unsafe { (*h).flags |= DEFERRED };
                    m.header(h);
                    continue;
                }
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                unsafe { (*h).flags = ((*h).flags & !(FIN | DEFERRED)) | FINALIZED };
                self.tobefnz.push(h);
                m.header(h);
                self.finalize.swap_remove(i);
            } else {
                // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
                unsafe { (*h).flags &= !DEFERRED };
            }
        }
    }

    /// Register a table for finalization (a live `__gc` metamethod was just set
    /// via setmetatable). No-op if it is already pending a finalize (FIN bit).
    /// PUC 5.5 reference manual §2.5.3: "An object can be marked again for
    /// finalization by calling setmetatable with a different metatable, or
    /// with the same metatable but with a different __gc field" — so a
    /// previously finalized object (FINALIZED bit set then reset by
    /// `take_tobefnz`) can re-register. PUC's `luaC_checkfinalizer` is gated
    /// on `tofinalize(o)` only, which mirrors checking the FIN bit.
    pub(crate) fn register_finalizable(&mut self, t: Gc<Table>) {
        let h = t.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if (*h).flags & FIN == 0 {
                (*h).flags |= FIN;
                self.finalize.push(h);
            }
        }
    }

    /// Register a userdata for finalization. PUC 5.1 `newproxy(true)` plus a
    /// metatable carrying `__gc` lets a Lua script attach a finalizer to a
    /// proxy object — gc.lua's "testing userdata" section binds this
    /// behaviour together with weak tables.
    pub(crate) fn register_finalizable_userdata(&mut self, u: Gc<crate::runtime::Userdata>) {
        let h = u.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if (*h).flags & FIN == 0 {
                (*h).flags |= FIN;
                self.finalize.push(h);
            }
        }
    }

    /// Every userdata still awaiting finalization, registered or already
    /// queued. The io library uses it to reach all open files the way C's
    /// `fflush(NULL)` and `exit` reach every `FILE*`.
    pub(crate) fn finalizable_userdata(&self) -> Vec<Gc<crate::runtime::Userdata>> {
        self.finalize
            .iter()
            .chain(self.tobefnz.iter())
            // SAFETY: both lists hold GcHeader pointers of live objects registered for finalization (heap.rs:5-7); a finalizable object is not freed before its finalizer runs.
            .filter(|&&h| unsafe { (*h).tag } == ObjTag::Userdata)
            .map(|&h| Gc::from_ptr(h as *mut crate::runtime::Userdata))
            .collect()
    }

    /// Take the objects awaiting their `__gc` call (the VM runs the
    /// finalizers). Each entry is dispatched on its `ObjTag` so the caller
    /// can look up `__gc` for either a table or a proxy userdata.
    ///
    /// Mirrors PUC 5.5 `udata2finalize`: the FINALIZED bit is reset on the
    /// way out so a `setmetatable(obj, mt_with___gc)` inside (or after) the
    /// finalizer can re-register the object for a future round, per the Lua
    /// 5.5 reference manual §2.5.3 re-finalize semantics.
    pub(crate) fn take_tobefnz(&mut self) -> Vec<crate::runtime::Value> {
        use crate::runtime::Value;
        std::mem::take(&mut self.tobefnz)
            .into_iter()
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            .map(|h| unsafe {
                (*h).flags &= !FINALIZED;
                match (*h).tag {
                    ObjTag::Table => Value::Table(Gc::from_ptr(h as *mut Table)),
                    ObjTag::Userdata => {
                        Value::Userdata(Gc::from_ptr(h as *mut crate::runtime::Userdata))
                    }
                    _ => unreachable!("non-finalizable object queued for finalization"),
                }
            })
            .collect()
    }

    /// Move ALL still-registered finalizables to the pending queue regardless of
    /// reachability (PUC separatetobefnz(g, 1) at state close), so the VM can run
    /// every `__gc` before the heap is torn down.
    pub(crate) fn queue_all_finalizers(&mut self) {
        for h in std::mem::take(&mut self.finalize) {
            // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
            unsafe { (*h).flags = ((*h).flags & !FIN) | FINALIZED };
            self.tobefnz.push(h);
        }
    }
}
