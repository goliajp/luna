//! Finalizer bookkeeping: registering objects with `__gc` and queueing them.

use super::*;

impl Heap {
    /// The atomic step: every object with a finalizer found unreachable
    /// goes to `tobefnz`, marked through `m` so that it and what its
    /// finalizer needs survive this cycle. Returns PUC's size of what
    /// moved (5.1 `luaC_separateudata`'s).
    pub(super) fn separate_finalizables(&mut self, m: &mut Marker) -> i64 {
        self.separate(Some(m), false)
    }

    /// Whether an object with header flags `flags` can be registered now:
    /// not while registered, nor, in 5.2, once it has been finalized.
    fn may_register(&self, flags: u8) -> bool {
        flags & FIN == 0 && (self.rules.refinalize() || flags & FINALIZED == 0)
    }

    /// A table gets a finalizer: a metatable with `__gc` was just set
    /// (`luaC_checkfinalizer`). Not while it has one (on `finobj` or
    /// `tobefnz`); once its finalizer was taken, from 5.3 on, again
    /// (reference manual §2.5.3), 5.2 never.
    pub(crate) fn register_finalizable(&mut self, t: Gc<Table>) {
        self.get_finalizer(t.as_ptr() as *mut GcHeader);
    }

    /// [`Heap::register_finalizable`] for a userdata (5.1: every userdata
    /// that gets a metatable, whose `__gc` the atomic step looks for).
    pub(crate) fn register_finalizable_userdata(&mut self, u: Gc<crate::runtime::Userdata>) {
        self.get_finalizer(u.as_ptr() as *mut GcHeader);
    }

    fn get_finalizer(&mut self, h: *mut GcHeader) {
        // SAFETY: `h` heads a table or userdata the caller holds, so it is
        // allocated and on `all`
        unsafe {
            if self.may_register((*h).flags) {
                self.move_to_finobj(h);
            }
        }
    }

    /// Every userdata of the heap, as the state closes.
    pub(crate) fn all_userdata(&self) -> Vec<Gc<crate::runtime::Userdata>> {
        self.userdata_where(|_| true)
    }

    /// Every userdata with a finalizer still to run: the io library
    /// reaches every open file through them, as C's `fflush(NULL)` reaches
    /// every `FILE*`.
    pub(crate) fn finalizable_userdata(&self) -> Vec<Gc<crate::runtime::Userdata>> {
        self.userdata_where(|f| f & FIN != 0)
    }

    /// The userdata whose flags pass `keep`, oldest first.
    fn userdata_where(&self, keep: impl Fn(u8) -> bool) -> Vec<Gc<crate::runtime::Userdata>> {
        let mut out = Vec::new();
        let lists = self.object_lists();
        for (i, mut cur) in lists.into_iter().enumerate() {
            let start = out.len();
            // SAFETY: the lists link allocated objects; the tag says which
            // are userdata
            unsafe {
                while !cur.is_null() {
                    if (*cur).tag == ObjTag::Userdata && keep((*cur).flags) {
                        out.push(Gc::from_ptr(cur as *mut crate::runtime::Userdata));
                    }
                    cur = (*cur).next;
                }
            }
            // each list has its newest first, but `tobefnz` (the last) the
            // next to finalize
            if i + 1 < lists.len() {
                out[start..].reverse();
            }
        }
        out
    }

    /// The value of `h`, just taken off `tobefnz`, handed out for its
    /// `__gc` call. From 5.3 on the object may get a finalizer again
    /// (`udata2finalize` resets its bit), 5.2's stays finalized.
    ///
    /// # Safety
    /// `h` was taken off `tobefnz`.
    unsafe fn finalizable_value(&mut self, h: *mut GcHeader) -> crate::runtime::Value {
        use crate::runtime::Value;
        // SAFETY: the caller's contract: the marking kept it, and it is a
        // table or a userdata, the two tags matched below; the caller roots
        // the returned handle until the finalizer has run
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

    /// Every object with a finalizer to `tobefnz`, reachable or not, as
    /// the state closes (`separatetobefnz(g, 1)`, 5.1
    /// `luaC_separateudata(L, 1)`).
    pub(crate) fn queue_all_finalizers(&mut self) {
        self.separate(None, true);
    }
}

impl Heap {
    /// How many objects PUC's `finobj` list holds: those registered for
    /// finalization.
    pub(crate) fn finobj_count(&self) -> i64 {
        self.finobj_len as i64
    }

    /// How many objects PUC's `tobefnz` list holds.
    pub(crate) fn tobefnz_count(&self) -> i64 {
        self.tobefnz_len as i64
    }

    /// Whether an object awaits its `__gc` call.
    pub(crate) fn has_tobefnz(&self) -> bool {
        !self.tobefnz.is_null()
    }

    /// Take the first object awaiting its `__gc` call (PUC
    /// `udata2finalize`); the caller roots it until the call is made.
    pub(crate) fn take_one_tobefnz(&mut self) -> Option<crate::runtime::Value> {
        let h = self.tobefnz_pop()?;
        // SAFETY: `h` was taken off `tobefnz` just now
        Some(unsafe { self.finalizable_value(h) })
    }
}
