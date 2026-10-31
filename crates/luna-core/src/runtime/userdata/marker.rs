//! What a userdata's trace marks through.

use crate::runtime::heap::Gc;
use crate::runtime::value::Value;

/// Public facade over the GC mark accumulator passed to
/// [`crate::vm::LuaUserdata::trace`].
///
/// Wraps the crate-internal `Marker` (private GC primitive in
/// `runtime::heap`) so embedders never see the gray-stack / weak-table
/// internals. Holds a mutable
/// borrow of the underlying marker for the duration of a single trace
/// call. Constructed only by the collector via the crate-internal
/// `__new_internal` constructor; embedders cannot synthesize one outside
/// a trace call.
///
/// ## Trace-method contract
///
/// Inside [`crate::vm::LuaUserdata::trace`] the embedder may **only**:
/// - call [`UserdataMarker::mark`] / [`UserdataMarker::mark_value`] on
///   `Gc<...>` handles / `Value`s reachable from `&self`
/// - read fields of `&self`
///
/// The embedder must **not** allocate new GC objects, reenter the `Vm`,
/// take locks, or perform I/O. The trace call runs synchronously inside
/// the collector's mark phase and must return in bounded wall time.
pub struct UserdataMarker<'a> {
    inner: &'a mut crate::runtime::heap::Marker,
}

impl<'a> UserdataMarker<'a> {
    /// Crate-internal constructor. Not part of the public API — only
    /// the collector (`Userdata::trace`) builds one.
    pub(crate) fn __new_internal(inner: &'a mut crate::runtime::heap::Marker) -> Self {
        UserdataMarker { inner }
    }

    /// Mark a Gc-managed object as reachable. Returns `true` on the
    /// first visit (white → gray transition). Idempotent on later
    /// visits within the same cycle.
    pub fn mark<T: crate::runtime::GcObject>(&mut self, g: Gc<T>) -> bool {
        self.inner.mark(g)
    }

    /// Convenience: mark every Gc-managed object referenced by a
    /// [`Value`]. No-op for primitive variants (`Int`, `Float`,
    /// `Bool`, `Nil`, `LightUserdata`).
    pub fn mark_value(&mut self, v: Value) -> bool {
        self.inner.value(v)
    }
}
