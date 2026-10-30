//! Writing a trace's registers back to the frame on its exit, with the
//! tags the exit analysis gives them.

use super::*;

impl Vm {
    /// Write the trace's registers back to the frame with the tags its exit
    /// analysis gives them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_restore_slots(
        &mut self,
        base_us: usize,
        max_stack: usize,
        keep_tfor: std::ops::Range<usize>,
        using_global_exit_tags: bool,
        global_tag_res_kind: crate::jit::trace::TagResKind,
        exit_tags_for_pc: &[crate::jit::trace::ExitTag],
        reg_state: &[i64],
        entry_tags: &[u8],
        child_ran: bool,
    ) {
        // At an inline cmp@d>0
        // side-exit, the helper has pushed N frames on
        // top of the trace head's frame and
        // `exit_tags_for_pc.len()` covers the full
        // window (caller + each inlined frame's
        // window). Slots beyond `max_stack` belong to
        // an inlined frame: their `Untouched` entries
        // default to Nil (no entry-tag fallback —
        // marshal-in only captured caller slots) and
        // we write to interp stack at `base + i` which
        // mirrors `op_offsets`-derived layout.
        let mut slot_count = exit_tags_for_pc.len();
        // The helper only extends vm.stack up to the deepest pushed frame's
        // window, but the exit_tags snapshot covers the trace's full
        // `window_size` (which includes depth-N+1 scratch slots that the
        // trace's IR may have written without a matching pushed frame).
        // Extend with Nil so the write at the tail doesn't panic; these slots
        // get overwritten by the writeback loop and won't leak meaningful
        // data past the pushed frames' R[0..max_stack) windows. When the
        // stack cannot grow, those scratch slots are left out: every pushed
        // frame's window is on the stack already.
        if !self.jit_ensure_stack(base_us + slot_count) {
            slot_count = self.stack.len().saturating_sub(base_us);
        }
        let exit_tags_for_pc = &exit_tags_for_pc[..slot_count];
        // Fast-path restore loop. When
        // we landed on the global `exit_tags`,
        // dispatch on the compile-time
        // classification: skip the loop entirely
        // for `AllUntouched`, do a tag-free
        // `Value::Int(...)` write per slot for
        // `AllInt`, otherwise fall through to the
        // general match-arm loop. site_id > 0
        // (inline frame mat) and per_exit_tags
        // hits always take the general path —
        // their per-side-exit shapes aren't
        // pre-classified yet.
        let fast_path_taken = if using_global_exit_tags && keep_tfor.is_empty() {
            match global_tag_res_kind {
                crate::jit::trace::TagResKind::AllUntouched => {
                    // No-op: vm.stack already
                    // matches the trace's post-
                    // entry state for these
                    // slots (entry values not
                    // overridden, or already
                    // spilled by helpers).
                    true
                }
                crate::jit::trace::TagResKind::AllInt => {
                    for i in 0..slot_count {
                        self.stack[base_us + i] = crate::runtime::Value::Int(reg_state[i]);
                    }
                    true
                }
                crate::jit::trace::TagResKind::Mixed => false,
            }
        } else {
            false
        };
        if !fast_path_taken {
            let frame = &mut self.stack[base_us..base_us + slot_count];
            let regs = &reg_state[..slot_count];
            for (i, &exit_tag) in exit_tags_for_pc.iter().enumerate() {
                let mut tag = RAW_OF[exit_tag as usize];
                // `Untouched` first: a trace leaves most slots unwritten,
                // and a written boolean is rare
                if tag == UNTOUCHED {
                    if i >= max_stack {
                        tag = raw::NIL;
                    } else {
                        // not written: the stack holds the value it entered
                        // with, unless a side trace ran after the trace that
                        // wrote it
                        if !child_ran {
                            continue;
                        }
                        tag = entry_tags[i];
                        // not checked on entry and not written since: the
                        // stack still holds the value
                        if tag == crate::jit::trace::ENTRY_TAG_ANY {
                            continue;
                        }
                    }
                } else if tag == BOOL {
                    tag = raw::FALSE + (regs[i] & 1) as u8;
                }
                if keep_tfor.contains(&i) {
                    continue;
                }
                put(&mut frame[i], tag, regs[i]);
            }
        }
    }
}

use crate::jit::trace::ExitTag;
use crate::runtime::value::raw;

/// `Untouched` has no raw tag.
const UNTOUCHED: u8 = u8::MAX;
/// A boolean's tag is FALSE plus its payload.
const BOOL: u8 = u8::MAX - 1;
/// The raw tag each exit tag writes, by discriminant (a table load where a
/// `match` per slot was an indirect branch).
const RAW_OF: [u8; 9] = {
    let mut m = [0; 9];
    m[ExitTag::Untouched as usize] = UNTOUCHED;
    m[ExitTag::Int as usize] = raw::INT;
    m[ExitTag::Float as usize] = raw::FLOAT;
    m[ExitTag::Table as usize] = raw::TABLE;
    m[ExitTag::Closure as usize] = raw::CLOSURE;
    // written nil (LoadNil): a nil whatever the entry tag
    m[ExitTag::Nil as usize] = raw::NIL;
    m[ExitTag::Str as usize] = raw::STR;
    m[ExitTag::Bool as usize] = BOOL;
    m[ExitTag::Native as usize] = raw::NATIVE;
    m
};

/// Writes the value of tag `tag` and payload word `r` to `dst`. The tags
/// the write-back passes are a slot's entry tag (checked on entry) or the
/// kind the exit analysis pins the register's payload to.
#[inline(always)]
fn put(dst: &mut Value, tag: u8, r: i64) {
    // SAFETY: as the doc says, `tag` is the kind of the value whose payload
    // `r` is, and that value is alive (on the stack or in the trace's
    // registers)
    unsafe { Value::pack_into(dst, tag, crate::runtime::value::RawVal { zero: r as u64 }) };
}
