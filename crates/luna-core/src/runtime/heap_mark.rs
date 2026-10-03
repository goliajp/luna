//! The marker: the gray stack and the entry points that color objects.

use super::*;

/// Mark accumulator: gray stack plus entry points for Values and bare
/// object headers (Protos/Upvalues are not first-class Values).
pub(crate) struct Marker {
    pub(super) stack: Vec<*mut GcHeader>,
    /// live tables with a weak `__mode`, collected during marking and processed
    /// (dead weak entries cleared) before the sweep
    pub(crate) weak: Vec<*mut Table>,
    /// ephemeron tables (weak keys, strong values): their hash values are not
    /// marked during trace but in a fixpoint pass keyed on key-reachability
    pub(crate) ephemeron: Vec<*mut Table>,
    /// PUC 5.1 mode: skip ephemeron handling — `__mode='k'` tables mark their
    /// values strongly during the normal trace pass (see [`Heap::no_ephemeron`]).
    pub(crate) no_ephemeron: bool,
    /// Protos with a non-null closure cache (PUC `Proto.cache`). After
    /// marking is done, any cached LClosure that ended the cycle unmarked is
    /// cleared so the sweep can collect it — the cache is a *weak* reference
    /// (PUC `traverseproto` checks `iswhite(cache)`). Seen via [`Proto::trace`].
    pub(crate) cached_protos: Vec<*mut crate::runtime::Proto>,
    /// Mark objects without children BLACK on sight instead of queueing
    /// them (PUC `reallymarkobject` does so for strings; its library
    /// functions are not objects at all). Only the stop-the-world mark sets
    /// it: the incremental step budget counts queued objects, and leaves
    /// leaving the queue would change how far each step gets. `LEAF` or 0,
    /// tested against an object's flags.
    pub(super) leaf_black: u8,
}

/// Drain the gray stack: pop each marked object and trace its children until
/// the worklist is empty (iterative, so deep graphs don't overflow the Rust
/// stack). Shared by the root mark and the post-resurrection remark.
pub(super) fn drain_marker(m: &mut Marker) {
    while let Some(h) = m.stack.pop() {
        // SAFETY: `h` was popped off the gray stack, which only `Marker::header` and `barrier_back` push to, with headers of allocated objects; nothing is freed while marking, and the tag names the type to trace it as
        unsafe {
            // PUC `propagatemark`: gray → black before scanning children, so a
            // child that points back at us (cycle) re-traces us as already
            // black and does not loop. White bits were cleared on push.
            (*h).flags = ((*h).flags & !WHITE_BITS) | BLACK;
            match (*h).tag {
                ObjTag::Str => {}
                ObjTag::Table => (*(h as *mut Table)).trace(m),
                ObjTag::Proto => (*(h as *mut Proto)).trace(m),
                ObjTag::Closure => (*(h as *mut LuaClosure)).trace(m),
                ObjTag::Upvalue => (*(h as *mut Upvalue)).trace(m),
                ObjTag::Native => (*(h as *mut NativeClosure)).trace(m),
                ObjTag::Coro => (*(h as *mut crate::runtime::Coro)).trace(m),
                ObjTag::Userdata => (*(h as *mut Userdata)).trace(m),
            }
        }
    }
}

impl Marker {
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

    /// Mark a bare header, returning true if it was newly marked (was white).
    /// Transitions white → gray (in PUC `reallymarkobject` terms): clears the
    /// current-white bit and pushes onto the gray stack. `drain_marker` later
    /// pops it, traces children, and stamps it BLACK. With `leaf_black` set
    /// an object without children (a string, a native function without
    /// upvalues) goes straight to BLACK instead.
    ///
    /// # Safety
    /// `h` is the header of an object this heap allocated and has not freed.
    #[inline(always)]
    pub(crate) unsafe fn header(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: `h` heads an allocated object (the caller's contract), and nothing is freed while marking; only the flag byte is touched
        unsafe {
            let f = (*h).flags;
            if is_white(f) {
                if f & self.leaf_black != 0 {
                    (*h).flags = (f & !WHITE_BITS) | BLACK;
                } else {
                    (*h).flags = f & !WHITE_BITS;
                    self.stack.push(h);
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
