//! A closed recording that did not become an installed trace.

use super::*;

impl Vm {
    /// `record` did not compile with `opts`: counted as a failure of its
    /// head, and published to the other Vms sharing traces when `shared`
    /// gives the dialect they compile for.
    pub(super) fn trace_closed_failed(
        &mut self,
        record: &crate::jit::trace::TraceRecord,
        opts: crate::jit::trace::CompileOptions,
        shared: Option<LuaVersion>,
    ) {
        self.jit.counters.compile_failed += 1;
        if let Some(version) = shared {
            let jit = &mut self.jit;
            jit.trace_compiler
                .publish_failure(jit.storage.as_mut(), record, opts, version);
        }
        use super::super::entry_side::{note_recording_failure, note_recording_never_entered};
        let head_proto = record.head_proto;
        if self.jit.trace_compiler.last_compile_checkpoint() == "bail:entry-tag-never-entered" {
            note_recording_never_entered(&mut self.jit.counters, record);
        } else {
            note_recording_failure(&mut self.jit.counters, record);
            // an entry side trace's failures are its root's (the fail
            // memory would have the head skipped in code loaded later)
            if !record.entry_side {
                let m = self.jit.fail_memory.get_or_insert_default();
                m.failed(&head_proto, record.head_pc);
            }
        }
        self.jit
            .counters
            .compile_failed_reasons
            .push(self.jit.trace_compiler.last_compile_checkpoint());
    }

    /// A trace compiled that there is no room to run (see
    /// [`Vm::trace_room`]): not installed. Memory runs short for a while,
    /// so its head is not given up for it: recorded again once hot.
    pub(super) fn trace_closed_no_room(&mut self) {
        self.jit.counters.install_no_room += 1;
    }
}
