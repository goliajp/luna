//! The write-back after a side trace ran from its parent's exit, by the
//! plan of the side trace's exit (`SideHeld::after`), in place of the
//! per-register one.

use super::{BOOL, RAW_OF, put};
use crate::jit::trace_types::installed::side_held::ChildExit;
use crate::runtime::value::raw;
use crate::vm::Vm;

impl Vm {
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
