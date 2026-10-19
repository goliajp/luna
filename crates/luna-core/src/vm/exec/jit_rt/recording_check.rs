//! The objects a trace being recorded went through, and, in the builds
//! that check the collector (`gc-stress`, `gc-verify`), a check tests turn
//! on: after a collection, a recording still under way holds none the
//! collection freed or a pending sweep frees.

use super::*;
use crate::jit::trace::TraceRecord;
use crate::runtime::heap::GcHeader;
#[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
use std::cell::Cell;

#[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
thread_local! {
    static CHECK: Cell<bool> = const { Cell::new(false) };
}

/// The prototypes and the string `rec` refers to, none of them kept alive
/// by it.
pub(super) fn recording_objects(rec: &TraceRecord) -> impl Iterator<Item = *const GcHeader> + '_ {
    let h = |p: Gc<crate::runtime::Proto>| p.as_ptr() as *const GcHeader;
    std::iter::once(rec.head_proto)
        .chain(rec.ops.iter().map(|o| o.proto))
        .chain(rec.side_trace_parent.map(|p| p.0))
        .chain(rec.retfs.iter().map(|r| r.proto))
        .chain(rec.downrec_close.as_ref().map(|d| d.target_proto))
        .map(h)
        .chain(rec.index_key.map(|s| s.as_ptr() as *const GcHeader))
}

#[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
impl Vm {
    /// Have every collection on this thread end by checking the trace
    /// being recorded, if any, holds only objects the collector keeps.
    #[doc(hidden)]
    pub fn check_recordings_after_collections(on: bool) {
        CHECK.set(on);
    }

    /// The check [`Vm::check_recordings_after_collections`] turns on.
    pub(crate) fn check_recording_live(&self) {
        if !CHECK.get() {
            return;
        }
        let Some(rec) = self.jit.active_trace.as_deref() else {
            return;
        };
        for h in recording_objects(rec) {
            assert!(
                self.heap.holds_live(h),
                "a trace being recorded holds {h:p}, which the collection frees"
            );
        }
    }
}
