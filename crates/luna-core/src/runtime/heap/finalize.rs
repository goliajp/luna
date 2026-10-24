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
    /// Returns PUC's size of what it moved (5.1 `luaC_separateudata`'s).
    pub(super) fn separate_finalizables(&mut self, m: &mut Marker) -> i64 {
        let mut i = self.finalize.len();
        let mut queued = false;
        let mut dead = 0;
        while i > 0 {
            i -= 1;
            let h = self.finalize[i];
            // SAFETY: `finalize` holds headers of registered objects, and none is
            // freed while listed (an unreachable one is marked below before any
            // sweep); the header is only read here
            if unsafe { is_white((*h).flags) } {
                // 5.1 `luaC_separateudata`: a userdata whose metatable has
                // no `__gc` now is not finalized, and dies
                // SAFETY: `h` is a listed header, still allocated; a
                // userdata's tag says what it heads
                let no_gc_51 = self.gc_event_51.is_some_and(|name| unsafe {
                    (*h).tag == ObjTag::Userdata
                        && (*(h as *const crate::runtime::Userdata))
                            .metatable()
                            .is_none_or(|mt| mt.get_str(name).is_nil())
                });
                if no_gc_51 {
                    // SAFETY: as above; only the flag byte is written
                    unsafe { (*h).flags = ((*h).flags & !(FIN | DEFERRED)) | FINALIZED };
                    self.finalize[i] = ptr::null_mut();
                    queued = true;
                    continue;
                }
                // Two-pass cycle-finalize (PUC 5.3 gc.lua :502): when a
                // finalizable table holds onto an unreachable coroutine, the
                // cycle (table → coroutine.stack → closure → table) keeps the
                // mark phase from reaching the table even though it is still
                // logically alive for one more GC pass. PUC's mark-sweep wakes
                // it via `markbeingfnz` *after* sweeping, so the actual `__gc`
                // call lands one cycle later. luna mirrors this by resurrecting
                // the table on the first sighting and only enqueuing it for
                // `__gc` on the second.
                let in_thread_cycle = self.rules.late_thread_cycle_finalize()
                    // SAFETY: `h` is a listed header, still allocated (see
                    // above); only its tag byte is read
                    && unsafe { (*h).tag } == ObjTag::Table
                    && {
                        let t = h as *mut Table;
                        // SAFETY: the tag says `h` heads a table, still
                        // allocated (see above); the call only reads it
                        unsafe { (*t).refs_contain_unmarked_coro() }
                    };
                // SAFETY: `h` is a listed header, still allocated (see above);
                // only its flag byte is read
                let already_deferred = unsafe { (*h).flags & DEFERRED != 0 };
                if in_thread_cycle && !already_deferred {
                    // SAFETY: `h` is the allocated header read above; only its
                    // flag byte is written
                    unsafe { (*h).flags |= DEFERRED };
                    // SAFETY: `h` is the allocated header read above
                    unsafe { m.header(h) };
                    continue;
                }
                // SAFETY: `h` is the allocated header read above; only its flag
                // byte is written
                unsafe { (*h).flags = ((*h).flags & !(FIN | DEFERRED)) | FINALIZED };
                // 5.1 moves it off the list it sweeps until its finalizer
                // runs (`tmudata`), which `DEFERRED` stands for there
                if self.gc_event_51.is_some() {
                    // SAFETY: as above
                    unsafe { (*h).flags |= DEFERRED };
                }
                // SAFETY: as above
                dead += unsafe { self.puc_objsize(h) } as i64;
                self.queue_to_finalize(h);
                self.finalize[i] = ptr::null_mut();
                queued = true;
                // SAFETY: `h` is the allocated header read above
                unsafe { m.header(h) };
            } else {
                // SAFETY: `h` is the allocated header read above; only its flag
                // byte is written
                unsafe { (*h).flags &= !DEFERRED };
            }
        }
        // drop the queued ones in place: the survivors keep their order,
        // which decides the order of their finalizers in a later cycle
        if queued {
            self.finalize.retain(|h| !h.is_null());
        }
        dead
    }

    /// List `h` (not yet listed) for finalization, keeping room in
    /// `tobefnz` for every listed object: a collection moves them there and
    /// must not have to allocate to do it.
    ///
    /// # Safety
    /// `h` heads an allocated table or userdata the caller holds.
    unsafe fn register(&mut self, h: *mut GcHeader) -> Result<(), crate::runtime::mem::Oom> {
        let room = self.finalize.len() + 1 + self.tobefnz.len();
        if self.tobefnz.capacity() < room {
            self.tobefnz.reserve(room - self.tobefnz.len())?;
        }
        self.finalize.push(h)?;
        // SAFETY: the caller's contract; only the flag byte is written
        unsafe { (*h).flags |= FIN };
        Ok(())
    }

    /// Whether an object with header flags `flags` can be registered now:
    /// not while registered, nor, in 5.2, once it has been finalized.
    fn may_register(&self, flags: u8) -> bool {
        flags & FIN == 0 && (self.rules.refinalize() || flags & FINALIZED == 0)
    }

    /// Put `h` on `tobefnz`, whose room `register` keeps.
    fn queue_to_finalize(&mut self, h: *mut GcHeader) {
        debug_assert!(self.tobefnz.len() < self.tobefnz.capacity());
        // `register` keeps the room, so this never grows the queue
        let pushed = self.tobefnz.push(h);
        debug_assert!(pushed.is_ok());
    }

    /// Register a table for finalization (a live `__gc` metamethod was just set
    /// via setmetatable). No-op if it is already pending a finalize (FIN bit).
    /// PUC 5.5 reference manual §2.5.3: "An object can be marked again for
    /// finalization by calling setmetatable with a different metatable, or
    /// with the same metatable but with a different __gc field" — so a
    /// previously finalized object (FINALIZED bit set then reset by
    /// `take_tobefnz`) can re-register. PUC's `luaC_checkfinalizer` is gated
    /// on `tofinalize(o)` only, which mirrors checking the FIN bit.
    pub(crate) fn register_finalizable(
        &mut self,
        t: Gc<Table>,
    ) -> Result<(), crate::runtime::mem::Oom> {
        let h = t.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is the header of `t`, a table the caller holds a live handle to; only the flag byte is touched
        unsafe {
            if self.may_register((*h).flags) {
                return self.register(h);
            }
        }
        Ok(())
    }

    /// Register a userdata for finalization. PUC 5.1 `newproxy(true)` plus a
    /// metatable carrying `__gc` lets a Lua script attach a finalizer to a
    /// proxy object — gc.lua's "testing userdata" section binds this
    /// behaviour together with weak tables.
    pub(crate) fn register_finalizable_userdata(
        &mut self,
        u: Gc<crate::runtime::Userdata>,
    ) -> Result<(), crate::runtime::mem::Oom> {
        let h = u.as_ptr() as *mut GcHeader;
        // SAFETY: `h` is the header of `u`, a userdata the caller holds a live handle to; only the flag byte is touched
        unsafe {
            if self.may_register((*h).flags) {
                return self.register(h);
            }
        }
        Ok(())
    }

    /// Every userdata still awaiting finalization, registered or already
    /// queued. The io library uses it to reach all open files the way C's
    /// `fflush(NULL)` and `exit` reach every `FILE*`.
    /// Every userdata of the heap, as the state closes.
    pub(crate) fn all_userdata(&self) -> Vec<Gc<crate::runtime::Userdata>> {
        let mut out = Vec::new();
        let mut cur = self.all;
        // SAFETY: the links of `all` lead to live objects; the tag says
        // which are userdata
        unsafe {
            while !cur.is_null() {
                if (*cur).tag == ObjTag::Userdata {
                    out.push(Gc::from_ptr(cur as *mut crate::runtime::Userdata));
                }
                cur = (*cur).next;
            }
        }
        out
    }

    pub(crate) fn finalizable_userdata(&self) -> Vec<Gc<crate::runtime::Userdata>> {
        self.finalize
            .iter()
            .chain(self.tobefnz_live().iter())
            // SAFETY: `finalize` and `tobefnz` hold headers of registered objects, and the sweep never frees a listed one: `separate_finalizables` marks every unreachable entry before it can be swept, and an entry leaves the lists only through `take_tobefnz`; the tag says which entries are userdata
            .filter_map(|&h| unsafe {
                ((*h).tag == ObjTag::Userdata)
                    .then(|| Gc::from_ptr(h as *mut crate::runtime::Userdata))
            })
            .collect()
    }

    /// The value of the queued object `h`, handed out for its `__gc` call.
    ///
    /// Mirrors PUC 5.5 `udata2finalize`: the FINALIZED bit is reset on the
    /// way out so a `setmetatable(obj, mt_with___gc)` inside (or after) the
    /// finalizer can re-register the object for a future round, per the Lua
    /// 5.5 reference manual §2.5.3 re-finalize semantics.
    ///
    /// # Safety
    /// `h` was taken off `tobefnz`.
    unsafe fn finalizable_value(&mut self, h: *mut GcHeader) -> crate::runtime::Value {
        use crate::runtime::Value;
        // SAFETY: the caller's contract; every `tobefnz` entry was marked by `separate_finalizables` (or `mark_all` re-marks it) and so is still allocated; it was registered as a table or a userdata, the two tags matched below, and the caller roots the returned handle until the finalizer has run
        unsafe {
            if self.rules.refinalize() {
                (*h).flags &= !FINALIZED;
            }
            match (*h).tag {
                ObjTag::Table => Value::Table(Gc::from_ptr(h as *mut Table)),
                ObjTag::Userdata => {
                    Value::Userdata(Gc::from_ptr(h as *mut crate::runtime::Userdata))
                }
                _ => unreachable!("non-finalizable object queued for finalization"),
            }
        }
    }

    /// Move ALL still-registered finalizables to the pending queue regardless of
    /// reachability (PUC separatetobefnz(g, 1) at state close), so the VM can run
    /// every `__gc` before the heap is torn down.
    pub(crate) fn queue_all_finalizers(&mut self) {
        // newest first, as `separate_finalizables` orders them
        let all = self.finalize.take();
        for &h in all.iter().rev() {
            // SAFETY: `finalize` entries are registered objects the sweep has not freed (see `finalizable_userdata`); only the flag byte is written
            unsafe { (*h).flags = ((*h).flags & !FIN) | FINALIZED };
            self.queue_to_finalize(h);
        }
    }
}

impl Heap {
    /// How many objects PUC's `finobj` list holds: those registered for
    /// finalization.
    pub(crate) fn finobj_count(&self) -> i64 {
        self.finalize.len() as i64
    }

    /// How many objects PUC's `tobefnz` list holds.
    pub(crate) fn tobefnz_count(&self) -> i64 {
        self.tobefnz_live().len() as i64
    }

    /// The objects awaiting their `__gc` call, in the order they run.
    pub(crate) fn tobefnz_live(&self) -> &[*mut GcHeader] {
        &self.tobefnz[self.tobefnz_head..]
    }

    /// Whether an object awaits its `__gc` call.
    pub(crate) fn has_tobefnz(&self) -> bool {
        self.tobefnz_head < self.tobefnz.len()
    }

    /// Take the first object awaiting its `__gc` call (PUC
    /// `udata2finalize`); the caller roots it until the call is made.
    pub(crate) fn take_one_tobefnz(&mut self) -> Option<crate::runtime::Value> {
        let &h = self.tobefnz.get(self.tobefnz_head)?;
        self.tobefnz_head += 1;
        // 5.1's `GCTM` puts it back on the list it sweeps
        // SAFETY: a queued object is allocated until its finalizer has run
        unsafe { (*h).flags &= !DEFERRED };
        let n = self.tobefnz.len();
        if self.tobefnz_head == n {
            self.tobefnz.clear();
            self.tobefnz_head = 0;
        } else if self.tobefnz_head >= 64 && self.tobefnz_head * 2 >= n {
            // the handed-out front is dropped once it is half the queue
            self.tobefnz.copy_within(self.tobefnz_head..n, 0);
            self.tobefnz.truncate(n - self.tobefnz_head);
            self.tobefnz_head = 0;
        }
        // SAFETY: `h` was taken off `tobefnz` above
        Some(unsafe { self.finalizable_value(h) })
    }
}
