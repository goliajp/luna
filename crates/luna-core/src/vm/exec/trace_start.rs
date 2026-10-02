//! Starting a trace recording at a loop's back-edge.

use super::*;

impl Vm {
    /// A loop head `target` crossed often enough (see
    /// `JitState::loop_hot_tick`): start recording there unless a
    /// recording is running, the function gave up, or `target` already
    /// has a trace or was abandoned. `tfor` is the base register of a
    /// generic `for`. `true` when a recording started.
    #[inline(never)]
    pub(super) fn trace_start_at_back_edge(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        target: u32,
        tfor: Option<u32>,
    ) -> bool {
        let proto = cl.proto;
        // the cheap tests first, then the borrow and scan of the traces
        if self.jit.active_trace.is_some()
            || proto.trace_gave_up.get()
            || proto.traces.borrow().iter().any(|t| t.head_pc == target)
            || trace_head_abandoned(proto, target)
        {
            return false;
        }
        self.trace_start_at_loop(cl, base, target, tfor);
        true
    }

    /// Start recording at `target`, the first instruction of a loop body in
    /// the running frame. `tfor` is the base register of a generic `for`.
    #[inline(never)]
    pub(super) fn trace_start_at_loop(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        target: u32,
        tfor: Option<u32>,
    ) {
        // the tag of each register at entry tells the trace compiler which
        // arithmetic to lower (integer or float, and so on)
        let max_stack = cl.proto.max_stack as usize;
        let base_us = base as usize;
        let mut entry_tags = Vec::with_capacity(max_stack);
        for i in 0..max_stack {
            let (tag, _) = self.stack[base_us + i].unpack();
            entry_tags.push(tag);
        }
        let mut rec = crate::jit::trace::TraceRecord::start(cl.proto, target, entry_tags, false);
        if let Some(a) = tfor {
            // a native iterator's address lets the trace compiler
            // specialise `ipairs` into inline array reads
            rec.tfor_iter_ptr = match self.stack[base_us + a as usize] {
                Value::Native(n) => Some(n.f as usize),
                _ => None,
            };
            // the tag of the value the last `TForCall` produced: the inline
            // read guards on it, so an array of mixed tags leaves the trace
            let val_slot = base_us + a as usize + 5;
            rec.tfor_val_tag =
                (val_slot < self.stack.len()).then(|| self.stack[val_slot].unpack().0);
        }
        self.jit.active_trace = Some(Box::new(rec));
        // the running frame is the one the trace starts in
        self.jit.recording_frame_base = self.frames.len() - 1;
    }
}
