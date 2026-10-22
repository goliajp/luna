//! The marker: the gray stack and the entry points that color objects.

use super::*;

mod at;
mod notes;
pub(crate) use at::native_puc_upvals;

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
    /// The gray stack. When it cannot grow, the object stays gray off it
    /// and `overflow` is set; [`Marker::refill`] walks `scan` for them.
    pub(in crate::runtime::heap) stack: GrayStack,
    pub(in crate::runtime::heap) overflow: bool,
    /// The head of the heap's object list when the marker was lent out:
    /// every object that can be gray is on it (new ones are born white,
    /// fixed ones live on their own list).
    pub(in crate::runtime::heap) scan: *mut GcHeader,
    /// live tables with a weak `__mode`, collected during marking and processed
    /// (dead weak entries cleared) before the sweep
    pub(in crate::runtime::heap) weak: Vec<*mut Table>,
    /// ephemeron tables (weak keys, strong values): their hash values are not
    /// marked during trace but in a fixpoint pass keyed on key-reachability
    pub(in crate::runtime::heap) ephemeron: Vec<*mut Table>,
    /// PUC 5.1 mode: skip ephemeron handling — `__mode='k'` tables mark their
    /// values strongly during the normal trace pass (see [`Heap::no_ephemeron`]).
    pub(in crate::runtime::heap) no_ephemeron: bool,
    /// the dialect, whose `reallymarkobject` decides which objects are
    /// traversed as soon as they are marked
    pub(in crate::runtime::heap) dialect: crate::runtime::dialect::Dialect,
    /// Protos with a closure cache (PUC `Proto.cache`), a weak reference:
    /// a cached closure left unmarked is cleared once marking is done.
    pub(in crate::runtime::heap) cached_protos: Vec<*mut crate::runtime::Proto>,
    /// `LEAF` or 0, tested against an object's flags: objects without
    /// children go BLACK on sight (only the stop-the-world mark sets it; a
    /// native standing for a C closure has work an incremental step counts).
    pub(in crate::runtime::heap) leaf_black: u8,
    /// 5.2 and 5.3: the size of a string's header, to count what marking
    /// a string or a userdata adds to the work (PUC `GCmemtrav`); 0 for
    /// the other dialects
    pub(in crate::runtime::heap) leaf_hdr: usize,
    /// that work so far
    pub(in crate::runtime::heap) leaf_bytes: i64,
    /// 5.2: the size of a closed upvalue, whose marking is work too; else 0
    pub(in crate::runtime::heap) leaf_upval: usize,
    /// 5.5: counts what is marked
    pub(in crate::runtime::heap) sizer: Option<super::collect::Sizer>,
    /// luna's own objects reached, gray off the stack until
    /// [`Marker::drain_quiet`] marks what they hold
    pub(in crate::runtime::heap) quiet: Vec<*mut GcHeader>,
    /// marking what `quiet` holds: nothing is counted
    pub(in crate::runtime::heap) quiet_mode: bool,
    /// 5.1's environment records, or null
    pub(in crate::runtime) env51: *const super::env51::Env51,
    /// the loads a step runs inside (`PucModel::parse`), or null
    pub(in crate::runtime::heap) parse: *const Vec<super::parse::ParseFrame>,
    /// 5.2 and 5.3: the objects of PUC's luna's stand for, or null
    pub(in crate::runtime::heap) stands: *const Vec<(*mut GcHeader, usize)>,
    /// `gc-verify`: instead of marking a child, report one the sweep about
    /// to run would free (see `gc_hooks::ChildCheck`)
    pub(in crate::runtime::heap) verify: crate::runtime::gc_hooks::ChildCheck,
}

/// Drain the gray stack: pop each marked object and trace its children until
/// the worklist is empty (iterative, so deep graphs don't overflow the Rust
/// stack). Shared by the root mark and the post-resurrection remark.
pub(super) fn drain_marker(m: &mut Marker) {
    while let Some(h) = m.pop() {
        // SAFETY: `h` was popped off the gray stack, which only `Marker::header` and `barrier_back` push to, with headers of allocated objects; nothing is freed while marking
        unsafe { blacken(h, m) };
    }
    m.drain_quiet();
}

/// PUC `propagatemark`: `h` gray → black before tracing its children, so a
/// child that points back at it (a cycle) finds it black.
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
            ObjTag::Str => {}
            ObjTag::Ghost => {
                if super::env51::is_phantom(h) {
                    m.trace_phantom(h);
                } else if super::parse::is_load_tab(h) {
                    m.trace_load_tab(h);
                }
            }
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

    /// Mark a bare header (PUC `reallymarkobject`), true if it was white:
    /// gray, on the gray stack; a string, and with `leaf_black` set any
    /// object without children, straight to BLACK.
    ///
    /// # Safety
    /// `h` is the header of an object this heap allocated and has not freed.
    #[inline(always)]
    pub(crate) unsafe fn header(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: as below
        if unsafe { self.verify.report(h) } {
            return false;
        }
        // SAFETY: `h` heads an allocated object (the caller's contract), and nothing is freed while marking; only the flag byte is touched
        unsafe {
            let f = (*h).flags;
            if is_white(f) {
                if !self.quiet_mode {
                    if self.luna_only(h) {
                        (*h).flags = (*h).with_slow(f & !WHITE_BITS);
                        self.quiet.push(h);
                        return true;
                    }
                    if self.leaf_hdr != 0 {
                        self.count_leaf(h);
                    }
                    if let Some(s) = &mut self.sizer {
                        s.marked += s.size(h);
                    }
                }
                // a string has nothing to traverse and no work of its own
                // (its bytes went to its parent's above): black at once,
                // as PUC's `reallymarkobject` makes it
                if f & self.leaf_black != 0 || (*h).tag == ObjTag::Str {
                    (*h).flags = (*h).with_slow((f & !WHITE_BITS) | BLACK);
                    (*h).cls &= !super::parse::kept_bit((*h).tag);
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
