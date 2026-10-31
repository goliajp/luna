//! What a weak table's clearing asks of an entry: whether its key or its
//! value survived the marking.

use super::*;

/// Whether a value is "alive" for ephemeron key purposes: non-collectable
/// values and strings are always alive (strings are never weakly cleared);
/// a collectable object is alive only once marked (gray or black).
pub(in crate::runtime::heap) fn weak_key_alive(v: Value) -> bool {
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
pub(in crate::runtime::heap) fn survived_mark(v: Value) -> bool {
    match v {
        Value::Str(s) => !is_white(s.hdr.flags),
        v => weak_key_alive(v),
    }
}
