//! After a dispatch: the links compiled code took (see
//! `crate::vm::exec::trace::relink`), and the counts of the dispatch.

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::exit_code::EXIT_RELINK;
use crate::jit::trace_types::frame::trace_ran_side_slot;
use crate::jit::trace_types::side_cell::LINK_TAG_KEPT;

impl Vm {
    /// After the code of a dispatch returned `r`: when its last trace (`ct`,
    /// or the last one linked to since the trail's `start`) ran a side
    /// trace that ended at the head of the trace it links to, but the code
    /// left the link (`LINK_DEPTH` taken in a row), the link taken here, the
    /// count of links in a row started again: the entry to call next on the
    /// same buffer. `None` when the dispatch is done.
    pub(super) fn trace_link_again(
        &mut self,
        proto: Gc<crate::runtime::Proto>,
        ct: &CompiledTrace,
        start: usize,
        r: i64,
        regs: &mut [i64],
    ) -> Option<crate::jit::trace::TraceFn> {
        let raw = r as u64;
        if raw & EXIT_RELINK == 0 {
            return None;
        }
        let last = self.jit.link_trail[start..].last().map(|l| l[2] as usize);
        let held = last.map(|i| TArc::clone(&proto.traces.borrow()[i]));
        let ran = held.as_deref().unwrap_or(ct);
        if !ran.has_side_cells() {
            return None;
        }
        let typed = u32::try_from(regs[trace_ran_side_slot(ran.window_size as usize)])
            .ok()?
            .checked_sub(1)?;
        let cell = ran.side_cell(typed)?;
        if cell.link_pc.get() != raw as u32 {
            return None;
        }
        let call: [u32; 3] = cell.link_lists.borrow().0.get(3..6)?.try_into().ok()?;
        self.jit.link_base = self.jit.link_trail.len();
        let target = self.link_take(&call, regs, false)?;
        self.jit.counters.links_redispatched += 1;
        Some(self.trace_entry_counted(&target, proto.call_hot_count.get()))
    }

    /// The links taken since this dispatch began, which ran on its buffers,
    /// forgotten, the innermost dispatch's start put back to `outer`: the
    /// last trace linked to, whose return the dispatch decodes as though it
    /// had entered it, with `entry_tags` made the tags it was entered with.
    /// `None` when no link was taken.
    pub(super) fn trace_links_taken(
        &mut self,
        proto: Gc<crate::runtime::Proto>,
        outer: usize,
        from: usize,
        entry_tags: &mut [u8],
    ) -> Option<TArc<CompiledTrace>> {
        self.jit.link_base = outer;
        if self.jit.link_trail.len() == from {
            return None;
        }
        let traces = proto.traces.borrow();
        let mut last = None;
        for &[typed, parent, to] in &self.jit.link_trail[from..] {
            let parent = &traces[parent as usize];
            let cell = (parent.side_cell(typed)).expect("a link taken from a cell its trace holds");
            for (t, &k) in entry_tags.iter_mut().zip(cell.link_lists.borrow().3.iter()) {
                if k != LINK_TAG_KEPT {
                    *t = k;
                }
            }
            last = Some(to);
        }
        self.jit.counters.links += (self.jit.link_trail.len() - from) as u64;
        self.jit.link_trail.truncate(from);
        last.map(|to| TArc::clone(&traces[to as usize]))
    }

    /// The counts of a dispatch of `ct` (the trace whose code returned).
    pub(super) fn trace_count_dispatch(&mut self, ct: &CompiledTrace, downrec: bool) {
        self.jit.counters.dispatched += 1;
        if ct.inline_kinds != 0 {
            self.count_inline_kinds(ct.inline_kinds);
        }
        // A helper that could not do its op (a metatable, no memory) parked
        // a deopt and the trace left at that op: the exit is where the
        // interpreter goes on, as for any guard, and it does the op itself.
        // The mark stays up while the exit is restored (no side trace runs
        // from it).
        if self.jit.pending_err().is_some() {
            self.jit.counters.deopt += 1;
            if downrec {
                self.jit.counters.downrec_deopt += 1;
            }
        }
    }
}
