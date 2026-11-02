//! The trace JIT dispatcher: entering a compiled trace at the pc the
//! interpreter is about to run, and resuming the interpreter where it exits.

use super::*;
use downrec::downrec_close_exit;

impl Vm {
    /// Run the trace cached for `pc` of `cl` (frame `base`), if one admits this
    /// entry. `true` when the trace ran and the frame's pc now points past it.
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_dispatch(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        base: u32,
        downrec_admit_blocked: bool,
    ) -> bool {
        let Some(ct) = ({
            let traces = cl.proto.traces.borrow();
            traces
                .iter()
                .find(|t| Vm::trace_admits(t, pc, downrec_admit_blocked))
                .cloned()
        }) else {
            return false;
        };
        // `ct` is held across the dispatch, so its fields are borrowed
        let entry_fn = self.trace_entry_counted(&ct, cl.proto.call_hot_count.get());
        let head_pc_val = ct.head_pc;
        let window_size = ct.window_size;
        let compile_entry_tags = &ct.entry_tags;
        let max_stack = cl.proto.max_stack as usize;
        let window_size_us = window_size as usize;
        let base_us = base as usize;
        // the Vm's buffers, taken for the dispatch and put back on every
        // way out; `reg_state` covers the trace's window, which may exceed
        // the frame
        let mut entry_tags: Vec<u8> = std::mem::take(&mut self.jit.entry_tags_buf);
        entry_tags.resize(max_stack, 0);
        // a trace that closes by down-recursion (whether or not it is also
        // dispatchable) guards on the caller's saved pc, in a slot just past
        // the window
        let is_downrec_entry = ct.downrec_link.is_some();
        let mut reg_state: Vec<i64> = std::mem::take(&mut self.jit.reg_state_buf);
        let reg_state_len = if is_downrec_entry {
            window_size_us + 1
        } else {
            window_size_us
        };
        // marshal-in fills the frame's registers; the rest start at zero
        reg_state.resize(reg_state_len, 0i64);
        reg_state[max_stack.min(reg_state_len)..].fill(0);
        let dispatch_ok = self.inlined_calls_fit(ct.inline_depth, base, ct.window_size)
            && self.trace_marshal_in(
                base_us,
                max_stack,
                compile_entry_tags,
                &mut entry_tags,
                &mut reg_state,
                !ct.has_any_side_wired.get(),
            );

        if dispatch_ok {
            debug_assert_eq!(head_pc_val, pc, "trace cache hit's head_pc != pc");
            // A recording in progress cannot see what the trace runs
            // natively: it would resume after the trace with ops
            // missing, and could close as a loop that never ran (a
            // side trace of two ops returning its own head, which
            // the dispatcher then entered forever). Drop it.
            // Counted as a failure of that head: the per-head hot count
            // would otherwise start the same doomed recording again and
            // again.
            if let Some(rec) = self.jit.active_trace.take() {
                self.jit.counters.aborted += 1;
                self.jit.counters.bump_close_cause("reached-compiled-trace");
                note_trace_compile_failure(&mut self.jit.counters, rec.head_proto, rec.head_pc);
            }
            self.jit.pending_err = None;
            // an inline side exit pushes the inlined frames, which a deopt
            // pops
            let pre_frames = self.frames.len();
            if is_downrec_entry {
                reg_state[window_size_us] = self.downrec_saved_pc(pre_frames);
            }
            // `LUNA_AOT_PROBE`: one line at the first dispatch, for the AOT
            // smoke test to see compiled code ran
            if jit_probe_enabled() && self.jit.counters.dispatched == 0 {
                eprintln!("luna-runtime-helpers: aot_trace_fired pc={head_pc_val}");
            }
            self.jit_ghosts_enter();
            let running = self.trace_step_enter(&ct);
            let continuation_pc = {
                let vm_ptr: *mut Vm = self;
                let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                // SAFETY: `entry_fn` is the entry of `ct`, a compiled trace `cl.proto.traces` keeps alive; `reg_state` was sized above to the trace's window (plus the saved-pc slot for a down-recursion entry) and filled by `trace_marshal_in`; the guard above pins this Vm and `cl` for the helpers the trace calls
                unsafe { entry_fn(reg_state.as_mut_ptr()) }
            };
            self.trace_step_leave(running);
            self.jit.ghost_depth -= 1;
            self.jit.counters.dispatched += 1;
            if ct.inline_kinds != 0 {
                self.count_inline_kinds(ct.inline_kinds);
            }

            if self.jit.pending_err.is_some() {
                self.jit.pending_err = None;
                self.jit.step.synced_exit = false;
                self.jit.gc_exit = None;
                self.jit.counters.deopt += 1;
                // unwind the inlined frames helpers pushed; the trace's
                // writes are discarded, the interpreter runs from `pc`
                while self.frames.len() > pre_frames {
                    self.pop_frame();
                }
                if is_downrec_entry {
                    // as a guard miss: the next admit is suppressed so the
                    // interpreter gets past `head_pc`
                    self.jit.counters.downrec_deopt += 1;
                    self.jit.suppress_downrec_admit_once = true;
                }
            } else if is_downrec_entry && downrec_close_exit(continuation_pc, head_pc_val) {
                // a due step leaves through an exit, never the close
                debug_assert!(self.jit.gc_exit.is_none());
                self.trace_downrec_classify(continuation_pc, pre_frames);
                self.jit.reg_state_buf = reg_state;
                self.jit.entry_tags_buf = entry_tags;
                return true;
            } else {
                self.trace_exit_restore(
                    cl,
                    base,
                    &ct,
                    continuation_pc,
                    pre_frames,
                    reg_state,
                    entry_tags,
                );
                return true;
            }
        }
        self.jit.reg_state_buf = reg_state;
        self.jit.entry_tags_buf = entry_tags;
        false
    }

    /// Copy the frame's registers into the trace's entry buffer. `false` when a
    /// register the trace checks has a tag other than the one the trace was
    /// compiled for, or one that cannot be passed as a raw payload.
    /// `checked_only`: copy only the registers the trace checks. The others
    /// are held on the stack (see the lowering's `StackHeld`), and only a
    /// side trace run from one of its exits reads them, or their entry tags,
    /// from the buffers.
    fn trace_marshal_in(
        &self,
        base_us: usize,
        max_stack: usize,
        compile_entry_tags: &[u8],
        entry_tags: &mut [u8],
        reg_state: &mut [i64],
        checked_only: bool,
    ) -> bool {
        // one loop per value of `checked_only`: tested once per slot, it
        // cost two instructions on every register of every entry
        if checked_only {
            self.marshal_in_slots::<true>(
                base_us,
                max_stack,
                compile_entry_tags,
                entry_tags,
                reg_state,
            )
        } else {
            self.marshal_in_slots::<false>(
                base_us,
                max_stack,
                compile_entry_tags,
                entry_tags,
                reg_state,
            )
        }
    }

    #[inline(always)]
    fn marshal_in_slots<const CHECKED_ONLY: bool>(
        &self,
        base_us: usize,
        max_stack: usize,
        compile_entry_tags: &[u8],
        entry_tags: &mut [u8],
        reg_state: &mut [i64],
    ) -> bool {
        use crate::jit::trace::{ENTRY_TAG_ANY, PLAIN_ENTRY_TAGS};
        let frame = &self.stack[base_us..base_us + max_stack];
        // the slots the trace was compiled with tags for, then the ones past
        // them: zipped, so no slot pays for a bounds check
        let n = compile_entry_tags.len().min(max_stack);
        let (regs, regs_past) = reg_state[..max_stack].split_at_mut(n);
        let (tags, tags_past) = entry_tags[..max_stack].split_at_mut(n);
        let slots = frame[..n].iter().zip(&compile_entry_tags[..n]);
        for ((v, &want), (r, t)) in slots.zip(regs.iter_mut().zip(tags.iter_mut())) {
            if want == ENTRY_TAG_ANY {
                // not read before the trace writes it: any value enters,
                // and an exit that has not written it leaves it as it is
                if !CHECKED_ONLY {
                    *r = unpack_word(*v).1;
                    *t = ENTRY_TAG_ANY;
                }
                continue;
            }
            let (tag, payload) = unpack_word(*v);
            *r = payload;
            // The trace's IR is specialised to the compile-time entry
            // tags: on another, body ops would misread the raw bits (a Str
            // pointer as an Int payload). The interpreter runs this entry;
            // the trace stays for later ones. The payload of anything else
            // cannot stand for the value. A boolean, the one entry type
            // two tags enter, is left to the out-of-line test, so that the
            // common case costs one comparison and one bit test
            if tag != want || PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
                match bool_entry(want, tag) {
                    Some(p) => *r = p,
                    None => return false,
                }
            }
            *t = tag;
        }
        marshal_past(&frame[n..], regs_past, tags_past)
    }
}

/// `v`'s tag and its payload word as a register holds it.
#[inline(always)]
fn unpack_word(v: Value) -> (u8, i64) {
    let (tag, payload) = v.unpack();
    // SAFETY: every field of `RawVal` is one plain 8-byte word, and
    // `unpack` returns it fully initialised
    (tag, unsafe { payload.zero as i64 })
}

/// The payload a register of tag `tag` enters a trace with whose entry tag
/// for it is `want`, when `tag` is not the plain match of `want`: a trace
/// compiled for a boolean takes either value, as payload 0 or 1. `None`:
/// the register cannot enter.
#[cold]
#[inline(never)]
fn bool_entry(want: u8, tag: u8) -> Option<i64> {
    use crate::runtime::value::raw;
    (want == raw::FALSE && (tag == raw::FALSE || tag == raw::TRUE)).then(|| i64::from(tag - want))
}

/// [`Vm::marshal_in_slots`] of the slots past the compile-time tags, each
/// checked like one read.
#[inline(always)]
fn marshal_past(frame: &[Value], regs: &mut [i64], tags: &mut [u8]) -> bool {
    for (v, (r, t)) in frame.iter().zip(regs.iter_mut().zip(tags.iter_mut())) {
        let (tag, payload) = unpack_word(*v);
        *r = payload;
        if crate::jit::trace::PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
            match bool_entry(tag, tag) {
                Some(p) => *r = p,
                None => return false,
            }
        }
        *t = tag;
    }
    true
}
