//! A side trace whose recording reaches the head of a compiled trace in the
//! frame it began in closes there, linked to that trace, instead of being
//! dropped: it returns that head, where the dispatcher enters the trace.

use super::*;
use crate::runtime::Proto;

impl Vm {
    /// The dispatcher is about to enter the compiled trace at `pc` of
    /// `proto` while a recording runs. A recording that cannot go on
    /// past it (it would resume after the trace with the trace's ops
    /// missing) closes linked to it when it is a side trace in its own
    /// frame, else is dropped, counted as a failure of its head: the hot
    /// count would otherwise start the same recording again and again.
    pub(in crate::vm::exec) fn trace_reached_compiled(&mut self, proto: Gc<Proto>, pc: u32) {
        let in_frame = self.frames.len() == self.jit.recording_frame_base + 1;
        let Some(rec) = self.jit.active_trace.as_mut() else {
            return;
        };
        if rec.side_trace_parent.is_some()
            && in_frame
            && !rec.ops.is_empty()
            && rec.head_proto.ptr_eq(proto)
        {
            rec.link_pc = Some(pc);
            self.trace_close_recording();
            return;
        }
        let rec = self.jit.active_trace.take().expect("recording");
        self.jit.counters.aborted += 1;
        self.jit.counters.bump_close_cause("reached-compiled-trace");
        note_trace_compile_failure(&mut self.jit.counters, rec.head_proto, rec.head_pc);
    }
}
