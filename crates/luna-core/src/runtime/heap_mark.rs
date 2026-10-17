//! The marker: the gray stack and the entry points that color objects.

use super::*;

/// The gray stack, outside the allocation context: PUC links gray
/// objects through themselves, so marking allocates nothing a host
/// allocator or memory policy could see or refuse.
#[derive(Default)]
pub(crate) struct GrayStack(Vec<*mut GcHeader>);

impl GrayStack {
    pub(super) fn push(&mut self, h: *mut GcHeader) -> Result<(), ()> {
        self.0.try_reserve(1).map_err(drop)?;
        self.0.push(h);
        Ok(())
    }

    pub(super) fn pop(&mut self) -> Option<*mut GcHeader> {
        self.0.pop()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn take(&mut self) -> GrayStack {
        std::mem::take(self)
    }
}

/// Mark accumulator: gray stack plus entry points for Values and bare
/// object headers (Protos/Upvalues are not first-class Values).
pub(crate) struct Marker {
    /// The gray stack. When it cannot grow, the object stays gray off the
    /// stack and `overflow` is set; [`Marker::refill`] finds such objects
    /// again by walking `scan`.
    pub(super) stack: GrayStack,
    pub(super) overflow: bool,
    /// The head of the heap's object list when the marker was lent out:
    /// every object that can be gray is on it (new ones are born white,
    /// fixed ones live on their own list).
    pub(super) scan: *mut GcHeader,
    /// live tables with a weak `__mode`, collected during marking and processed
    /// (dead weak entries cleared) before the sweep
    pub(crate) weak: Vec<*mut Table>,
    /// ephemeron tables (weak keys, strong values): their hash values are not
    /// marked during trace but in a fixpoint pass keyed on key-reachability
    pub(crate) ephemeron: Vec<*mut Table>,
    /// PUC 5.1 mode: skip ephemeron handling — `__mode='k'` tables mark their
    /// values strongly during the normal trace pass (see [`Heap::no_ephemeron`]).
    pub(crate) no_ephemeron: bool,
    /// the dialect, whose `reallymarkobject` decides which objects are
    /// traversed as soon as they are marked
    pub(crate) dialect: crate::runtime::table::Dialect,
    /// Protos with a non-null closure cache (PUC `Proto.cache`). After
    /// marking is done, any cached LClosure that ended the cycle unmarked is
    /// cleared so the sweep can collect it — the cache is a *weak* reference
    /// (PUC `traverseproto` checks `iswhite(cache)`). Seen via [`Proto::trace`].
    pub(crate) cached_protos: Vec<*mut crate::runtime::Proto>,
    /// Mark objects without children BLACK on sight instead of queueing
    /// them (PUC's library functions are not objects at all). Only the
    /// stop-the-world mark sets it: a native standing for PUC's C closure
    /// has work of its own, which the incremental step must charge when it
    /// leaves the queue. Strings go BLACK on sight in every mark, as in
    /// PUC's `reallymarkobject`. `LEAF` or 0, tested against an object's
    /// flags.
    pub(super) leaf_black: u8,
    /// 5.2 and 5.3: the size of a string's header, to count what marking
    /// a string or a userdata adds to the work (PUC `GCmemtrav`); 0 for
    /// the other dialects
    pub(super) leaf_hdr: usize,
    /// that work so far
    pub(super) leaf_bytes: i64,
    /// 5.5: counts what is marked
    pub(super) sizer: Option<super::collect::Sizer>,
    /// `gc-verify`: instead of marking a child, report one the sweep about
    /// to run would free (white, not the current white) while the object
    /// being traced (the second) holds it; the check's name third
    #[cfg(feature = "gc-verify")]
    pub(super) verify: Option<(u8, *mut GcHeader, &'static str)>,
}

/// Drain the gray stack: pop each marked object and trace its children until
/// the worklist is empty (iterative, so deep graphs don't overflow the Rust
/// stack). Shared by the root mark and the post-resurrection remark.
pub(super) fn drain_marker(m: &mut Marker) {
    while let Some(h) = m.pop() {
        // SAFETY: `h` was popped off the gray stack, which only `Marker::header` and `barrier_back` push to, with headers of allocated objects; nothing is freed while marking
        unsafe { blacken(h, m) };
    }
}

/// PUC `propagatemark`: `h` gray → black before tracing its children, so a
/// child that points back at it (a cycle) finds it black and does not loop.
/// White bits were cleared when it was grayed.
///
/// # Safety
/// `h` is the header of a gray object, allocated until the sweep; the tag
/// names the type to trace it as.
#[inline(always)]
pub(super) unsafe fn blacken(h: *mut GcHeader, m: &mut Marker) {
    // SAFETY: the caller's contract
    unsafe {
        (*h).flags = (*h).with_slow(((*h).flags & !WHITE_BITS) | BLACK);
        trace_children(h, m);
    }
}

/// The children of `h` handed to `m`.
///
/// # Safety
/// As [`blacken`]'s.
#[inline(always)]
pub(super) unsafe fn trace_children(h: *mut GcHeader, m: &mut Marker) {
    // SAFETY: the caller's contract
    unsafe {
        match (*h).tag {
            ObjTag::Str | ObjTag::Ghost => {}
            ObjTag::Table => (*(h as *mut Table)).trace(h as *mut Table, m),
            ObjTag::Proto => (*(h as *mut Proto)).trace(h as *mut Proto, m),
            ObjTag::Closure => (*(h as *mut LuaClosure)).trace(m),
            ObjTag::Upvalue => (*(h as *mut Upvalue)).trace(m),
            ObjTag::Native => (*(h as *mut NativeClosure)).trace(m),
            ObjTag::Coro => (*(h as *mut crate::runtime::Coro)).trace(m),
            ObjTag::Userdata => (*(h as *mut Userdata)).trace(m),
        }
    }
}

impl Marker {
    /// The next gray object to trace: off the stack, or found again by a walk
    /// of the object list after the stack overflowed.
    #[inline]
    pub(super) fn pop(&mut self) -> Option<*mut GcHeader> {
        match self.stack.pop() {
            Some(h) => Some(h),
            None if self.overflow => self.refill(),
            None => None,
        }
    }

    /// Push `h`, already gray; when the stack cannot grow it stays gray and
    /// [`Marker::refill`] finds it later.
    #[inline(always)]
    pub(super) fn push_gray(&mut self, h: *mut GcHeader) {
        if self.stack.push(h).is_err() {
            self.overflow = true;
        }
    }

    /// Walk the object list for gray objects left off the stack when it
    /// could not grow, tracing each where it is found (a stack that cannot
    /// grow could not hold them), until a walk leaves none behind; then the
    /// next object on the stack.
    #[cold]
    #[inline(never)]
    fn refill(&mut self) -> Option<*mut GcHeader> {
        while self.overflow {
            self.overflow = false;
            let mut cur = self.scan;
            while !cur.is_null() {
                // SAFETY: `scan` heads the heap's object list, whose objects
                // stay allocated while marking (frees happen only in the
                // sweep), and each `next` is read from a live object
                let (f, next) = unsafe { ((*cur).flags, (*cur).next) };
                if !is_white(f) && !is_black(f) {
                    // SAFETY: `cur` is a gray object on the list (see above)
                    unsafe { blacken(cur, self) };
                }
                cur = next;
            }
            if let Some(h) = self.stack.pop() {
                return Some(h);
            }
        }
        None
    }

    /// 5.2 and 5.3 count a string's or a userdata's size as the work of
    /// marking it.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[cold]
    #[inline(never)]
    unsafe fn count_leaf(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            if (*h).sub & UNCOUNTED != 0 {
                return;
            }
            self.leaf_bytes += match (*h).tag {
                ObjTag::Str => (self.leaf_hdr + (*(h as *const LuaStr)).len() + 1) as i64,
                ObjTag::Userdata => (*(h as *const Userdata)).puc_size as i64,
                _ => 0,
            };
        }
    }

    /// Whether PUC marks `h` with what it holds as soon as it marks it.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    unsafe fn at_once(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        match unsafe { (*h).tag } {
            ObjTag::Upvalue => true,
            ObjTag::Userdata => self.dialect <= crate::runtime::table::Dialect::L53,
            _ => false,
        }
    }

    /// What a userdata `h` marked at once holds: its metatable, then its
    /// user value, a chain of userdata followed in a loop as PUC 5.3's
    /// `reallymarkobject` does, so that it takes no stack.
    ///
    /// # Safety
    /// `h` heads an allocated userdata, black already.
    unsafe fn userdata_at_once(&mut self, mut h: *mut GcHeader) {
        // SAFETY: the caller's contract; each userdata followed is held by
        // the one before, allocated while marking
        unsafe {
            loop {
                let u = &*(h as *const Userdata);
                if let Some(mt) = u.metatable() {
                    self.mark(mt);
                }
                if let UserdataPayload::Host {
                    data,
                    trace_fn: Some(trace_fn),
                    ..
                } = &u.payload
                {
                    let mut um = crate::vm::UserdataMarker::__new_internal(self);
                    trace_fn(data, &mut um);
                }
                let Value::Userdata(next) = u.user_value else {
                    self.value(u.user_value);
                    return;
                };
                let n = next.header();
                let f = (*n).flags;
                if !is_white(f) {
                    return;
                }
                if self.leaf_hdr != 0 {
                    self.count_leaf(n);
                }
                if let Some(s) = &mut self.sizer {
                    s.marked += s.size(n);
                }
                (*n).flags = (*n).with_slow((f & !WHITE_BITS) | BLACK);
                h = n;
            }
        }
    }

    /// Mark a value, returning true if it was newly marked (was white).
    pub(crate) fn value(&mut self, v: Value) -> bool {
        let h = match v {
            Value::Str(s) => s.header(),
            Value::Table(t) => t.header(),
            Value::Closure(c) => c.header(),
            Value::Native(n) => n.header(),
            Value::Coro(c) => c.header(),
            Value::Userdata(u) => u.header(),
            _ => return false,
        };
        // SAFETY: `h` is the header of the object `v` holds a handle to
        unsafe { self.header(h) }
    }

    /// [`Self::header`] for an object the caller holds a handle to.
    #[inline(always)]
    pub(crate) fn mark<T: GcObject>(&mut self, g: Gc<T>) -> bool {
        // SAFETY: a handle's object is allocated, and a `GcObject` starts with its header
        unsafe { self.header(g.header()) }
    }

    /// [`Self::mark`] of each string of `strs`: with nothing to count per
    /// object (5.1, 5.4), a white one turns black and readable only through
    /// the slow path, as `header` leaves it.
    pub(crate) fn mark_strings<'a>(&mut self, strs: impl Iterator<Item = &'a Gc<LuaStr>>) {
        #[cfg(feature = "gc-verify")]
        let plain = self.verify.is_none();
        #[cfg(not(feature = "gc-verify"))]
        let plain = true;
        if !plain || self.leaf_hdr != 0 || self.sizer.is_some() {
            for &s in strs {
                self.mark(s);
            }
            return;
        }
        for &s in strs {
            let h = s.header();
            // SAFETY: a handle's string is allocated and nothing is freed while marking; only its flag byte is touched
            unsafe {
                let f = (*h).flags;
                if is_white(f) {
                    (*h).flags = (f & !WHITE_BITS) | BLACK | SLOW;
                }
            }
        }
    }

    /// Mark a bare header, returning true if it was newly marked (was white).
    /// Transitions white → gray (in PUC `reallymarkobject` terms): clears the
    /// current-white bit and pushes onto the gray stack. `drain_marker` later
    /// pops it, traces children, and stamps it BLACK. A string, and with
    /// `leaf_black` set any object without children (a native function
    /// without upvalues), goes straight to BLACK instead.
    ///
    /// # Safety
    /// `h` is the header of an object this heap allocated and has not freed.
    #[inline(always)]
    pub(crate) unsafe fn header(&mut self, h: *mut GcHeader) -> bool {
        #[cfg(feature = "gc-verify")]
        if let Some(check) = self.verify {
            // SAFETY: as below
            unsafe { super::verify::dead_child(h, check) };
            return false;
        }
        // SAFETY: `h` heads an allocated object (the caller's contract), and nothing is freed while marking; only the flag byte is touched
        unsafe {
            let f = (*h).flags;
            if is_white(f) {
                if self.leaf_hdr != 0 {
                    self.count_leaf(h);
                }
                if let Some(s) = &mut self.sizer {
                    s.marked += s.size(h);
                }
                // a string has nothing to traverse and no work of its own
                // (its bytes went to its parent's above): black at once,
                // as PUC's `reallymarkobject` makes it
                if f & self.leaf_black != 0 || (*h).tag == ObjTag::Str {
                    (*h).flags = (*h).with_slow((f & !WHITE_BITS) | BLACK);
                } else if self.at_once(h) {
                    // an upvalue, and a userdata up to 5.3, are never gray:
                    // what they hold is marked as they are
                    (*h).flags = (*h).with_slow((f & !WHITE_BITS) | BLACK);
                    if (*h).tag == ObjTag::Userdata {
                        self.userdata_at_once(h);
                    } else {
                        trace_children(h, self);
                    }
                } else {
                    (*h).flags = (*h).with_slow(f & !WHITE_BITS);
                    self.push_gray(h);
                }
                true
            } else {
                false
            }
        }
    }
}

/// Whether a value is "alive" for ephemeron key purposes: non-collectable
/// values and strings are always alive (strings are never weakly cleared);
/// a collectable object is alive only once marked (gray or black).
pub(super) fn weak_key_alive(v: Value) -> bool {
    let h = match v {
        Value::Table(t) => t.as_ptr() as *mut GcHeader,
        Value::Closure(c) => c.as_ptr() as *mut GcHeader,
        Value::Native(n) => n.as_ptr() as *mut GcHeader,
        Value::Coro(c) => c.as_ptr() as *mut GcHeader,
        Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
        _ => return true, // strings, numbers, booleans: never weak-collected
    };
    // SAFETY: `v` is a key of a weak table being marked; the sweep that could free its object has not run, and only the flag byte is read
    unsafe { !is_white((*h).flags) }
}

/// Whether the object of `v` survived the marking just finished (a value
/// that is no object did): marked, or never collected (fixed).
pub(super) fn survived_mark(v: Value) -> bool {
    match v {
        Value::Str(s) => !is_white(s.hdr.flags),
        v => weak_key_alive(v),
    }
}
