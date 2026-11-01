//! A compiled trace as the dispatcher enters it.

use super::*;

/// A trace compiled by a backend and ready to be dispatched into at its
/// head pc: its [`CompiledTraceParts`], and the side traces wired to its
/// exits since. Owned by `Proto.traces`; the code is kept alive by the
/// Vm's JIT storage for the Vm's lifetime, like the method JIT's compiled
/// functions.
///
/// Read through a prototype's traces, a trace's parts cannot be changed;
/// the example compiles, the ones after it fail to.
///
/// ```
/// use luna_core::__private::{CompiledTrace, CompiledTraceParts, ProtoTraceInternals};
/// fn head(ct: &CompiledTrace) -> u32 { ct.head_pc + ct.tier_up.is_some() as u32 }
/// fn traces(p: &luna_core::runtime::Proto) -> usize { p.traces().len() }
/// // SAFETY: the caller vouches for the parts
/// unsafe fn make(p: CompiledTraceParts) -> CompiledTrace { unsafe { CompiledTrace::from_parts(p) } }
/// // SAFETY: as above
/// unsafe fn put(p: &luna_core::runtime::Proto) { unsafe { p.set_jit_next(None) } }
/// ```
///
/// ```compile_fail,E0616
/// fn wire(ct: &luna_core::__private::CompiledTrace) {
///     ct.side_children.borrow_mut().clear(); // error[E0616]: field `side_children` is private
/// }
/// ```
///
/// ```compile_fail,E0133
/// use luna_core::__private::{CompiledTrace, CompiledTraceParts};
/// fn make(p: CompiledTraceParts) -> CompiledTrace {
///     CompiledTrace::from_parts(p) // error[E0133]: `from_parts` is unsafe
/// }
/// ```
///
/// ```compile_fail,E0133
/// use luna_core::__private::ProtoTraceInternals;
/// fn put(p: &luna_core::runtime::Proto) {
///     p.set_jit_next(None); // error[E0133]: `set_jit_next` is unsafe
/// }
/// ```
pub struct CompiledTrace {
    pub(crate) parts: CompiledTraceParts,
    /// The exit index (as `exit_hit_counts`) of each side trace wired to
    /// this trace, by the sentinel code the IR would OR into a side
    /// trace's return (bits 56..=62). Read with [`Self::side_children`].
    pub(crate) side_trace_cache: TRefLock<std::collections::HashMap<u32, u32>>,
    /// The side traces wired to this trace's exits, by exit index. Held
    /// here rather than looked up on a proto: a side trace started at an
    /// exit inside a function the trace inlined is cached on that
    /// function's proto.
    pub(crate) side_children: TRefLock<std::collections::HashMap<u32, TArc<CompiledTrace>>>,
    /// Fast-path short-circuit hint for the
    /// dispatcher's tentative-decode + cell-load + check path. Set
    /// to `true` by the close handler when ANY of this trace's
    /// `exit_side_trace_ptrs` cells gets wired (i.e., the first
    /// time a child side trace compiles + the shape gate
    /// passes). Stays `true` for the trace's lifetime — once any
    /// side trace exists, the dispatcher must perform the per-
    /// exit check on every dispatch.
    ///
    /// When `false`, the dispatcher skips the tentative decode +
    /// cell load + child lookup entirely, falling straight through
    /// to the cheap parent decode + writeback. Trims fib_10_x10k-
    /// class tight-trace workloads' per-dispatch overhead from the
    /// double-decode pattern to a single `Cell::get()`.
    ///
    /// `Cell<bool>` so the close handler can flip the flag through
    /// only an `&CompiledTrace` borrow (the parent's `traces`
    /// borrow is immutable while the close handler walks).
    pub(crate) has_any_side_wired: TCellBool,
}

impl CompiledTrace {
    /// The trace the dispatcher enters, with no side trace wired yet.
    ///
    /// # Safety
    /// `parts.entry`, and the optimizing tier's entry its `tier_up` cells
    /// may hold, is code with the [`TraceFn`] ABI, compiled against the
    /// entry and exit tags and the window size `parts` states (the
    /// dispatcher moves registers in and out by them unchecked), that
    /// lives as long as any Vm the trace is installed on. The side-trace
    /// cells of `parts` are null.
    pub unsafe fn from_parts(parts: CompiledTraceParts) -> Self {
        Self {
            parts,
            side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
            side_children: TRefLock::new(std::collections::HashMap::new()),
            has_any_side_wired: TCellBool::new(false),
        }
    }

    /// The entry to call: the optimizing tier's once it exists.
    pub fn current_entry(&self) -> TraceFn {
        match &self.tier_up {
            Some(t) if !t.optimized.get().is_null() => {
                // SAFETY: `optimized` only ever holds an entry the backend
                // returned from `TraceCompiler::tier_up`, which has the
                // `TraceFn` ABI
                unsafe { std::mem::transmute::<*const u8, TraceFn>(t.optimized.get()) }
            }
            _ => self.entry,
        }
    }
}

impl std::fmt::Debug for CompiledTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledTrace")
            .field("head_pc", &self.head_pc)
            .field("n_ops", &self.n_ops)
            .field("dispatchable", &self.dispatchable)
            .field("exit_tags", &self.exit_tags)
            .field("entry", &"<fn>")
            .finish()
    }
}

impl std::ops::Deref for CompiledTrace {
    type Target = CompiledTraceParts;

    #[inline(always)]
    fn deref(&self) -> &CompiledTraceParts {
        &self.parts
    }
}
