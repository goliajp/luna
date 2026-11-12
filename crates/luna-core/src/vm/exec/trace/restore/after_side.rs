//! The write-back after a side trace ran from its parent's exit, by the
//! plan of the side trace's own exit (`Plans::exit`), in place of the
//! per-register one.

use super::{BOOL, RAW_OF, put};
use crate::jit::trace_types::installed::exit_plan::PlannedExit;
use crate::jit::trace_types::installed::side_held::SideHeld;
use crate::runtime::value::raw;
use crate::vm::Vm;

impl Vm {
    /// The write-back after `child`, a side trace that ran from the head
    /// frame's register 0 and left at its exit given with it, by the plan of
    /// that exit (`true`), when one applies: no generic `for` variables kept
    /// and the stack covers it. `at`: the frame's base, its registers and
    /// the number of exit tags the exit decoded to.
    #[inline(always)]
    pub(super) fn trace_restore_after(
        &mut self,
        child: Option<(&SideHeld, usize)>,
        keep_tfor: &std::ops::Range<usize>,
        at: (usize, usize, usize),
        reg_state: &[i64],
        entry_tags: &[u8],
    ) -> bool {
        let (base_us, max_stack, tags_len) = at;
        let Some((h, i)) = child else { return false };
        let Some(e) = h.child.plans.exit(i) else {
            return false;
        };
        if !keep_tfor.is_empty() || base_us + e.len > self.stack.len() {
            return false;
        }
        debug_assert_eq!(e.len, tags_len, "a side exit's plan of other tags");
        self.trace_restore_after_side(base_us, max_stack, e, reg_state, entry_tags);
        true
    }

    /// [`Self::trace_restore_slots`] after a side trace that starts at the
    /// head frame's register 0 ran and left at the exit `e` plans: the
    /// registers it wrote back with their kinds; the ones in the frame of
    /// `max_stack` registers it left untouched with the tag they entered
    /// the parent with (which may have written them), unless the parent
    /// took them unchecked; the ones past the frame it left untouched as
    /// nil. The caller checks that the stack covers `e.len` registers from
    /// `base_us` and that no generic `for` variables are kept.
    // out of line, as `trace_restore_planned`, for the dispatch's registers
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_restore_after_side(
        &mut self,
        base_us: usize,
        max_stack: usize,
        e: &PlannedExit,
        reg_state: &[i64],
        entry_tags: &[u8],
    ) {
        let frame = &mut self.stack[base_us..base_us + e.len];
        let mut written = e.written.iter().peekable();
        for (s, dst) in frame.iter_mut().enumerate() {
            let r = reg_state[s];
            if let Some(w) = written.next_if(|w| usize::from(w.slot) == s) {
                let mut tag = RAW_OF[w.kind as usize];
                if tag == BOOL {
                    tag = raw::FALSE + (r & 1) as u8;
                }
                put(dst, tag, r);
            } else if s >= max_stack {
                put(dst, raw::NIL, r);
            } else if entry_tags[s] != crate::jit::trace::ENTRY_TAG_ANY {
                put(dst, entry_tags[s], r);
            }
        }
    }
}
