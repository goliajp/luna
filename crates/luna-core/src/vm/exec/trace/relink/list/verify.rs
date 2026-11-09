//! What the baseline tier's call stub leaves when it took a link itself,
//! checked in a debug build against what `link_take` and
//! `trace_entry_counted` would have left.

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace_types::cell_plan::slot_bytes;
use crate::jit::trace_types::frame::{trace_count_slot, trace_passed_slot, trace_ran_side_slot};
use crate::jit::trace_types::side_cell::LINK_DEPTH;

impl Vm {
    /// Panics unless the stub's taking of `link` (`[typed exit, running
    /// trace, linked trace]`, run on `regs`) left what the Vm's would.
    pub(super) fn link_taken_agrees(&self, link: &[u32], regs: &[i64]) {
        let Some(CallFrame::Lua(f)) = self.frames.last() else {
            panic!("a link taken outside a Lua frame");
        };
        let (proto, base) = (f.closure.proto, f.base as usize);
        let (prev, target) = {
            let traces = proto.traces.borrow();
            let t = |i: u32| TArc::clone(&traces[i as usize]);
            (t(link[1]), t(link[2]))
        };
        let w = &self.jit.words;
        let len = w.trail_len();
        assert!(len > w.link_base() && len - w.link_base() <= LINK_DEPTH);
        assert_eq!(w.trail_get(len - 1), [link[0], link[1], link[2]]);
        assert_eq!(w.running.get().cast(), TArc::as_ptr(&target));
        let cell = prev.side_cell(link[0]).expect("a link from a cell");
        let lists = cell.link_lists.borrow();
        for &e in lists.1.get(3..).unwrap_or_default() {
            let slot = base + (e >> 8) as usize;
            assert_eq!(self.stack[slot].unpack().0, e as u8, "check of slot {slot}");
        }
        for &e in lists.2.get(3..).unwrap_or_default() {
            let (slot, d) = ((e >> 8) as usize, e as u8);
            if let Some(v) = restore::exit_value_of(d, regs[slot]) {
                let at = self.stack[base + slot];
                assert_eq!(slot_bytes(at), slot_bytes(v), "store of slot {slot}");
            }
        }
        drop(lists);
        counts_agree(&prev, &target, regs);
    }
}

/// The two traces' count words and the linked trace's move count as the
/// Vm leaves them after taking the link, once counted.
fn counts_agree(prev: &CompiledTrace, target: &CompiledTrace, regs: &[i64]) {
    let window = target.window_size as usize;
    let at = trace_count_slot(prev.window_size as usize);
    // the linked trace's count word, written after the running trace's
    // was read, takes its place when both windows put it in one slot
    let replaced = at == trace_count_slot(window) && target.call_count && target.tier_up.is_some();
    if prev.call_count
        && !replaced
        && let Some(t) = &prev.tier_up
    {
        assert_eq!(t.count.get(), regs[at] as u32, "the running trace's count");
    }
    if let Some(t) = &target.tier_up {
        let counted = u32::from(!t.tried.get());
        if target.call_count {
            let word = regs[trace_count_slot(window)] as u64;
            assert_eq!(word as u32, (word >> 32) as u32);
            assert_eq!(regs[trace_passed_slot(window)], 0);
            assert_eq!(t.count.get(), (word as u32).wrapping_add(counted));
        }
    }
    if target.has_side_cells() {
        assert_eq!(regs[trace_ran_side_slot(window)], 0);
    }
}
