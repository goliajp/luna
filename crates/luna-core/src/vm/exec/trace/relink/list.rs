//! The lists a side cell holds for the code: the checks before a side
//! trace runs and the stores when it goes on in a trace's loop.

use super::*;

/// What a list of [`SideList`] kind `kind` over `slots` holds for the code
/// (see `Vm::side_cell_list`): the kind, the number of slots, the registers
/// they reach, then `(slot << 8) | tag` for each; empty when there are
/// none.
pub(super) fn list_of(kind: SideList, slots: &[(usize, u8)]) -> Box<[u32]> {
    let Some(&(last, _)) = slots.last() else {
        return Box::new([]);
    };
    let mut out = vec![kind as u32, slots.len() as u32, last as u32 + 1];
    out.extend(slots.iter().map(|&(i, t)| (i as u32) << 8 | u32::from(t)));
    out.into()
}

/// What a side cell's list does.
#[derive(Clone, Copy)]
pub(super) enum SideList {
    /// Checks the stack slots hold the entry tags given.
    Check = 0,
    /// Stores the registers to the stack slots with the exit tags given.
    Store = 1,
    /// Enters the trace a side trace links to: three words, the typed exit,
    /// the running trace and the linked trace (see `Vm::link_enter`).
    Link = 2,
    /// As `Link`, the code having made the link's checks and stores.
    LinkBare = 3,
}

impl Vm {
    /// Runs a side cell's list (see [`list_of`]) on the frame on top, whose
    /// registers the trace's code holds in `reg_state`: 1 when the checks
    /// pass, or after the stores. An empty list is the checks with no slot
    /// to check. The checks are the dispatcher's before it runs a side
    /// trace (`exit_runs_no_side`, `child_reads_stack_held_ok`).
    pub(crate) fn side_cell_list(&mut self, list: &[u32], reg_state: &mut [i64]) -> i64 {
        debug_assert!(self.run_words_agree(), "the run words went stale");
        match list.first() {
            Some(&k) if k == SideList::Link as u32 => {
                return self.link_enter(&list[3..6], reg_state, false);
            }
            Some(&k) if k == SideList::LinkBare as u32 => {
                return self.link_enter(&list[3..6], reg_state, true);
            }
            _ => {}
        }
        let store = list.first() == Some(&(SideList::Store as u32));
        if !store
            && (self.jit.gc_exit().is_some()
                || self.jit.pending_err().is_some()
                || self.unmade.any())
        {
            return 0;
        }
        let Some(CallFrame::Lua(f)) = self.frames.last() else {
            return 0;
        };
        let base = f.base as usize;
        for &e in list.get(3..).unwrap_or_default() {
            let (slot, tag) = ((e >> 8) as usize, e as u8);
            if !store {
                if self.stack[base + slot].unpack().0 != tag {
                    return 0;
                }
            } else if let Some(v) = restore::exit_value_of(tag, reg_state[slot]) {
                self.stack[base + slot] = v;
            }
        }
        1
    }
}
