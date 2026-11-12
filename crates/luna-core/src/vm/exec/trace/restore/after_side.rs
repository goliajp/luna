//! The write-back after a side trace ran from its parent's exit, by the
//! plan of the side trace's exit (`SideHeld::after`), in place of the
//! per-register one.

use super::{BOOL, RAW_OF, put};
use crate::jit::trace_types::installed::side_held::{ChildExit, SideHeld};
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
        let Some(a) = h.after(i, max_stack) else {
            return false;
        };
        if !keep_tfor.is_empty() || base_us + a.len > self.stack.len() {
            return false;
        }
        debug_assert_eq!(a.len, tags_len, "a side exit's plan of other tags");
        self.trace_restore_after_side(base_us, a, reg_state, entry_tags);
        true
    }

    /// [`Self::trace_restore_slots`] after a side trace that starts at the
    /// head frame's register 0 ran and left at the exit `after` plans: the
    /// registers it wrote back with their kinds, and the ones in the frame
    /// it left untouched with the tag they entered the parent with (which
    /// may have written them), unless the parent took them unchecked. The
    /// caller checks that the stack covers `after.len` registers from
    /// `base_us` and that no generic `for` variables are kept.
    // out of line, as `trace_restore_planned`, for the dispatch's registers
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_restore_after_side(
        &mut self,
        base_us: usize,
        after: &ChildExit,
        reg_state: &[i64],
        entry_tags: &[u8],
    ) {
        let frame = &mut self.stack[base_us..base_us + after.len];
        for &(s, kind) in &after.written {
            let s = usize::from(s);
            let r = reg_state[s];
            let mut tag = RAW_OF[kind as usize];
            if tag == BOOL {
                tag = raw::FALSE + (r & 1) as u8;
            }
            put(&mut frame[s], tag, r);
        }
        for &s in &after.entered {
            let s = usize::from(s);
            let tag = entry_tags[s];
            if tag != crate::jit::trace::ENTRY_TAG_ANY {
                put(&mut frame[s], tag, reg_state[s]);
            }
        }
    }
}
