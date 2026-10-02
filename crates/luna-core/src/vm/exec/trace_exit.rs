//! Leaving a compiled trace: decode which exit it took (running a wired
//! side trace first), write the registers back with their tags and move the
//! frame to the exit's pc.

use super::*;
use crate::jit::trace::CompiledTrace;

impl Vm {
    /// The trace `ct` entered at `pc` returned `continuation_pc`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_exit_restore(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        base: u32,
        ct: &CompiledTrace,
        continuation_pc: i64,
        pre_frames: usize,
        mut reg_state: Vec<i64>,
        entry_tags: Vec<u8>,
    ) {
        let head_pc_val = ct.head_pc;
        let per_exit_inline = &ct.per_exit_inline;
        let global_tag_res_kind = ct.global_tag_res_kind;
        let max_stack = cl.proto.max_stack as usize;
        let base_us = base as usize;
        // Restore each slot using the trace's
        // exit-tag analysis (see ExitTag docs).
        // Decode the IR's
        // side-exit shape. Upper 32 bits = (site_idx
        // + 1) for inline cmp side-exits, 0 for
        // legacy clean-tail / non-inline exits.
        // The decode lives in
        // `crate::jit::trace::decode_exit_shape` so
        // side-trace returns can reuse it with the SIDE
        // TRACE's shape inputs when the sentinel bit
        // is set on `raw_ret`.
        let raw_ret = continuation_pc as u64;
        // Side-trace return decode.
        // Bit 63 of `raw_ret` is the side-trace
        // marker the parent's IR OR'd in when it
        // tail-called into a wired child trace.
        // Bits 56..=62 carry the sentinel code (the
        // cache key into the parent's
        // `side_trace_cache`); bits 0..=55 are the
        // child's own return value (encoded site or
        // plain cont_pc) which we MUST decode using
        // the CHILD's per_exit_inline / per_exit_tags
        // / exit_tags / exit_hit_counts — not the
        // parent's. The dispatcher snapshot read
        // above holds the parent's shapes; when bit
        // 63 is set we re-fetch the child's via the
        // sentinel-keyed cache.
        let from_side_trace = (raw_ret >> 63) & 1 == 1;
        let (child, decode_body, child_ran) =
            self.trace_exit_source(cl, pc, ct, raw_ret, &mut reg_state, base_us, &entry_tags);
        let shapes: &CompiledTrace = child.as_deref().unwrap_or(ct);
        let decode_inline = &shapes.per_exit_inline;
        let decode_hit_counts = &shapes.exit_hit_counts;
        let decoded = crate::jit::trace::decode_exit_shape(
            decode_body,
            decode_inline,
            &shapes.per_exit_tags,
            &shapes.exit_tags,
        );
        let site_id = decoded.site_id;
        let cont_pc = decoded.cont_pc;
        let exit_hit_idx = decoded.exit_hit_idx;
        let exit_tags_for_pc = decoded.exit_tags_for_pc;
        // When a side trace ran (tail-called by the
        // parent's code or invoked here), force
        // using_global_exit_tags=false so the restore
        // loop takes the per-tag slow path:
        // `global_tag_res_kind` classifies the parent's
        // exit tags, not the child's.
        let using_global_exit_tags = if child_ran {
            false
        } else {
            decoded.using_global_exit_tags
        };
        // Increment the counter (saturate
        // at u32::MAX to avoid wrap on long runs).
        // Track whether this increment is
        // the one that crossed `HOTEXIT_THRESHOLD`
        // (transition: previous v < threshold, new v
        // == threshold). The side-trace start is
        // deferred to just before `continue;` so
        // vm.stack and frame.pc are fully restored
        // (the snapshot reads post-restore values).
        let mut side_trace_should_start = false;
        // For side-trace returns the
        // counter to bump is the CHILD's (decoded
        // shape lookup) — `exit_hit_idx` is into the
        // decoded layout, so use the matching
        // `decode_hit_counts`. For parent decode
        // they're aliased (clone of the parent's
        // own Rc).
        let tier_exit = !child_ran && is_tier_exit(ct, cont_pc);
        if let Some(c) = decode_hit_counts.get(exit_hit_idx).filter(|_| !tier_exit) {
            let v = c.get();
            if v < u32::MAX {
                c.set(v + 1);
            }
            // After a side trace ran, `exit_hit_idx` is an
            // exit of that child, but a side trace is
            // recorded and wired as one of `head_pc_val`'s
            // exits: it would replace the side trace on the
            // parent's exit of that number, which resumes
            // elsewhere.
            if v + 1 == crate::jit::trace::HOTEXIT_THRESHOLD
                && !child_ran
                && self.jit.active_trace.is_none()
                && self.jit.trace_enabled
            {
                side_trace_should_start = true;
            }
        }
        self.trace_restore_slots(
            cl,
            base_us,
            max_stack,
            decode_body,
            cont_pc,
            using_global_exit_tags,
            global_tag_res_kind,
            exit_tags_for_pc,
            &reg_state,
            &entry_tags,
            child_ran,
        );
        // For non-inline exits the
        // helper was never called (no metas chain for
        // this cont_pc), so `frames.last()` is the
        // trace head's frame and we set its pc to
        // cont_pc as before. For inline exits the
        // helper baked the side-exit PC into the
        // innermost frame's `pc` at push time
        // (chain.last().pc was overridden at emit),
        // so this assignment to `frames.last_mut().pc
        // = cont_pc` is a redundant-but-correct
        // confirmation.
        let _ = &per_exit_inline; // hold the Rc alive across dispatch
        // For inline side-exits the
        // helper has pushed N frames on top. The trace
        // head frame is at `pre_frames - 1`; set its
        // pc to `head_resume_pc` so when the chain
        // eventually pops back to it, interp resumes
        // PAST the trace's depth-0 Op::Call instead of
        // restarting from `head_pc` and re-triggering
        // dispatch (infinite loop). The innermost
        // (helper-pushed) frame already has its pc
        // baked in at compile time, but we still
        // assign `cont_pc` below for parity with the
        // non-inline path (no-op).
        if site_id > 0 {
            let idx = (site_id - 1) as usize;
            let head_resume_pc = decode_inline[idx].head_resume_pc;
            if pre_frames > 0
                && let CallFrame::Lua(f) = &mut self.frames[pre_frames - 1]
            {
                f.pc = head_resume_pc;
            }
        }
        let frames_len_now = self.frames.len();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(fmut) => {
                if crate::jit::trace::v2c_probe_enabled() {
                    eprintln!(
                        "[v2c-set-pc] from_side={} sentinel_or_raw={:#018x} prev_pc={} new_cont_pc={} site_id={} frames.len={} pre_frames={} max_stack={}",
                        from_side_trace,
                        raw_ret,
                        fmut.pc,
                        cont_pc,
                        site_id,
                        frames_len_now,
                        pre_frames,
                        max_stack,
                    );
                }
                fmut.pc = cont_pc;
            }
            _ => unreachable!("Cont frame at trace dispatch"),
        }
        // Deferred side-trace start. The
        // increment block above flagged this exit's
        // hit count crossing HOTEXIT_THRESHOLD; now
        // that vm.stack is restored and frame.pc is
        // settled, snapshot entry_tags from the
        // resume frame's window and create the
        // recorder. The recorder's first push fires
        // on the next interp iteration at cont_pc.
        //
        // `head_proto` for the side trace = cl.proto
        // (trace JIT only inlines self-recursive
        // calls today, so cont_pc always lands in
        // the same proto as the parent). Frame base
        // is the resume frame (top of `self.frames`
        // — inline-pushed frames moved this).
        if side_trace_should_start {
            self.trace_start_side(cl, base_us, cont_pc, head_pc_val, exit_hit_idx);
        }
        // Put the dispatch buffers back
        // before the `continue;` so the next
        // dispatch picks up the same allocation.
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
    }

    /// Start recording a side trace at `cont_pc`, the exit of the trace at
    /// `head_pc_val` that just became hot.
    fn trace_start_side(
        &mut self,
        cl: Gc<LuaClosure>,
        base_us: usize,
        cont_pc: u32,
        head_pc_val: u32,
        exit_hit_idx: usize,
    ) {
        let (resume_base, resume_proto) = match self.frames.last() {
            Some(CallFrame::Lua(f)) => (f.base as usize, f.closure.proto),
            _ => (base_us, cl.proto),
        };
        let resume_max_stack = resume_proto.max_stack as usize;
        let mut side_entry_tags: Vec<u8> = Vec::with_capacity(resume_max_stack);
        // Extend stack if cont_pc's frame window
        // overhangs the current stack len (rare,
        // but inline-pushed frame stack writes
        // only covered the trace's writeback).
        if self.stack.len() < resume_base + resume_max_stack {
            self.stack
                .resize(resume_base + resume_max_stack, crate::runtime::Value::Nil);
        }
        for i in 0..resume_max_stack {
            let (tag, _) = self.stack[resume_base + i].unpack();
            side_entry_tags.push(tag);
        }
        self.jit.active_trace = Some(Box::new(crate::jit::trace::TraceRecord::start_side_trace(
            resume_proto,
            cont_pc,
            side_entry_tags,
            cl.proto,
            head_pc_val,
            exit_hit_idx,
        )));
        self.jit.recording_frame_base = self.frames.len() - 1;
        self.jit.counters.side_trace_started += 1;
    }
}

/// Whether `ct` left at `cont_pc` to move to the optimizing tier: that exit
/// is no guard failing, so it neither counts nor starts a side trace. Its
/// count equals `at` only between that exit and the next dispatch, which
/// tiers up first.
fn is_tier_exit(ct: &CompiledTrace, cont_pc: u32) -> bool {
    cont_pc == ct.head_pc
        && ct
            .tier_up
            .as_ref()
            .is_some_and(|t| !t.tried.get() && t.count.get() == t.at)
}
