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
            let reused = calls != t.calls_at && n >= t.reused_at;
            if n >= t.at || reused {
                self.trace_tier_up(ct);
            }
        }
    }

    /// Has the backend compile `ct`'s baseline code again so that its
    /// calling exits run the side traces their cells hold, once one is
    /// filled while the trace runs that code (see
    /// `TraceCompiler::compile_calls`): asked once, and counted as a move
    /// again of a trace that moves to the optimizing tier.
    #[cold]
    pub(in crate::vm::exec) fn trace_compile_calls(
        &mut self,
        ct: &crate::jit::trace::CompiledTrace,
    ) {
        if ct.calls_compiled.get() {
            return;
        }
        if let Some(t) = &ct.tier_up
            && (!t.optimized.get().is_null()
                || t.retiers.get() >= crate::jit::trace_types::tier_up::RETIER_CAP)
        {
            return;
        }
        ct.calls_compiled.set(true);
        let entry = {
            let jit = &mut self.jit;
            jit.trace_compiler.compile_calls(jit.storage.as_mut(), ct)
        };
        let Some(e) = entry else { return };
        ct.entry.set(e as *const u8);
        if let Some(t) = &ct.tier_up {
            t.retiers.set(t.retiers.get() + 1);
        }
        self.jit.counters.calls_compiled += 1;
    }

    /// Hands a hot trace to the optimizing tier, or one there again (see
    /// `TierUp::retier`), and points everything that enters it at the new
    /// code.
    #[cold]
    pub(super) fn trace_tier_up(&mut self, ct: &crate::jit::trace::CompiledTrace) {
        let Some(t) = &ct.tier_up else { return };
        t.tried.set(true);
        let (entry, kept) = {
            let jit = &mut self.jit;
            let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
            jit.trace_compiler.tier_up_keeping(storage, ct)
        };
        // a backend still compiling better code keeps something in
        // `source`: ask again at the next entry when the trace runs the
        // optimizing tier's code, which does not count iterations, else once
        // the baseline code's count reaches `at` anew
        let busy = t.source.borrow().is_some();
        if busy {
            let counting = entry.is_none() && t.optimized.get().is_null();
            t.count
                .set(if counting { 0 } else { t.at.saturating_sub(1) });
            t.tried.set(false);
        }
        if let Some(entry) = entry {
            t.optimized.set(entry as *const () as *const u8);
            match t.retiering.get() {
                true => self.jit.counters.retiered += 1,
                false => self.jit.counters.tiered_up += 1,
            }
        }
        if !busy {
            // the optimizing tier's code does not count: the count stays
            // where the move left it
            if t.retiering.get() {
                t.retiering.set(false);
                t.count.set(t.at.saturating_sub(1));
            }
            if let Some(k) = kept {
                *t.kept.borrow_mut() = Some(k.source);
                if k.stale {
                    t.retier();
                }
            }
        }
    }
}
