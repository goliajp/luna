//! What a dispatch counts: inlined kinds, and the iterations that move a
//! trace to the optimizing tier.

use super::*;

impl Vm {
    #[cold]
    pub(super) fn count_inline_kinds(&mut self, kinds: u8) {
        for (b, n) in self
            .jit
            .counters
            .inline_kind_dispatched
            .iter_mut()
            .enumerate()
        {
            if kinds & (1 << b) != 0 {
                *n += 1;
            }
        }
    }

    /// The entry to call for `ct`, counting the entry towards its move to
    /// the optimizing tier and making that move when it is due. `calls`:
    /// the head function's `call_hot_count` now.
    pub(super) fn trace_entry_counted(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
        calls: u32,
    ) -> crate::jit::trace::TraceFn {
        self.count_towards_tier_up(ct, calls);
        ct.current_entry()
    }

    /// Counts an entry of `ct` towards its move to the optimizing tier and
    /// makes the move when it is due. `calls`: the head function's
    /// `call_hot_count` now.
    pub(in crate::vm::exec) fn count_towards_tier_up(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
        calls: u32,
    ) {
        if let Some(t) = &ct.tier_up
            && !t.tried.get()
        {
            let n = t.count.get().wrapping_add(1);
            t.count.set(n);
            let reused =
                calls != t.calls_at && n >= t.at / crate::jit::trace::TIER_UP_REUSED_DIVISOR;
            if n >= t.at || reused {
                self.trace_tier_up(ct);
            }
        }
    }

    /// Hands a hot trace to the optimizing tier and points everything that
    /// enters it at the new code.
    #[cold]
    pub(super) fn trace_tier_up(&mut self, ct: &crate::jit::trace::CompiledTrace) {
        let Some(t) = &ct.tier_up else { return };
        t.tried.set(true);
        let entry = {
            let jit = &mut self.jit;
            let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
            jit.trace_compiler.tier_up(storage, ct)
        };
        // a backend still compiling better code keeps something in
        // `source`: ask again at the next entry when the trace runs the
        // optimizing tier's code, which does not count iterations, else once
        // the baseline code's count reaches `at` anew
        if t.source.borrow().is_some() {
            let counting = entry.is_none() && t.optimized.get().is_null();
            t.count
                .set(if counting { 0 } else { t.at.saturating_sub(1) });
            t.tried.set(false);
        }
        let Some(entry) = entry else { return };
        let p = entry as *const () as *const u8;
        t.optimized.set(p);
        let c = t.parent_cell.get() as *const crate::jit::send_compat::TCellPtr;
        if !c.is_null() {
            // SAFETY: the cell belongs to the parent trace, which stays in
            // its proto's `traces` as long as this child can run
            unsafe { (*c).set(p) };
        }
        self.jit.counters.tiered_up += 1;
    }
}
