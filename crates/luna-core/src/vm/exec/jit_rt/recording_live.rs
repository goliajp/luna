//! In the builds that check the collector (`gc-stress`, `gc-verify`), a
//! check tests turn on: after a collection, a recording still under way
//! holds none the collection freed or a pending sweep frees.

use super::recording_check::recording_objects;
use super::*;
use std::cell::Cell;

thread_local! {
    static CHECK: Cell<bool> = const { Cell::new(false) };
}

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
