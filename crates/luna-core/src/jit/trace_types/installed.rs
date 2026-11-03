//! A compiled trace as the dispatcher enters it.

use super::tier_up::TierUp;
use super::*;

mod read;

/// A trace compiled by a backend and ready to be dispatched into at its
/// head pc: its [`CompiledTraceParts`], and the side traces wired to its
/// exits since. Owned by `Proto.traces`; the code is kept alive by the
/// Vm's JIT storage for the Vm's lifetime, like the method JIT's compiled
/// functions.
///
/// Once made, a trace's parts are read only through methods: nothing safe
/// can change them, nor give it an optimizing-tier source (taking the
/// source only keeps the trace at its tier). The example compiles, the
/// ones after it fail to.
///
/// ```
/// use luna_core::__private::{CompiledTrace, CompiledTraceParts, ProtoTraceInternals};
/// use luna_core::__private::{put_tier_source, take_tier_source};
/// fn head(ct: &CompiledTrace) -> u32 { ct.head_pc() + ct.tier_up().is_some() as u32 }
/// fn traces(p: &luna_core::runtime::Proto) -> usize { p.traces().len() }
/// fn keep(ct: &CompiledTrace) -> bool { take_tier_source(ct).is_some() }
/// // the side-trace cells are read, not rewired
/// fn cells(ct: &CompiledTrace) -> bool {
///     ct.global_side_trace_ptr().get().is_null() && ct.exit_side_trace_ptrs().is_empty()
/// }
/// // SAFETY: the caller vouches for the source
/// unsafe fn back(ct: &CompiledTrace) {
///     if let Some(s) = take_tier_source(ct) { unsafe { put_tier_source(ct, s) } }
/// }
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
/// ```compile_fail,E0616
/// fn swap(a: &luna_core::__private::CompiledTrace, b: &luna_core::__private::CompiledTrace) {
///     let s = b.tier_up().unwrap().source.borrow_mut().take(); // error[E0616]: field `source` is private
///     *a.tier_up().unwrap().source.borrow_mut() = s;
/// }
/// ```
///
/// ```compile_fail,E0616
/// fn cell(ct: &luna_core::__private::CompiledTrace) {
///     let _ = &ct.global_side_trace_ptr; // error[E0616]: field `global_side_trace_ptr` is private
/// }
/// ```
///
/// ```compile_fail,E0133
/// use luna_core::__private::{CompiledTrace, put_tier_source, take_tier_source};
/// fn back(ct: &CompiledTrace) {
///     if let Some(s) = take_tier_source(ct) {
///         put_tier_source(ct, s) // error[E0133]: `put_tier_source` is unsafe
///     }
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
    pub(crate) head_pc: u32,
    pub(crate) entry: TraceFn,
    pub(crate) n_ops: u32,
    pub(crate) dispatchable: bool,
    pub(crate) window_size: u32,
    pub(crate) exit_tags: TArc<[ExitTag]>,
    pub(crate) global_tag_res_kind: TagResKind,
    pub(crate) entry_tags: TArc<[u8]>,
    pub(crate) per_exit_tags: ExitTagTable,
    pub(crate) per_exit_inline: TArc<[InlineSideExit]>,
    pub(crate) exit_hit_counts: TArc<[TCellU32]>,
    pub(crate) exit_side_trace_ptrs: TArc<[TCellPtr]>,
    pub(crate) tags_side_trace_ptrs: Box<[TCellPtr]>,
    pub(crate) global_side_trace_ptr: Box<TCellPtr>,
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
    pub(crate) is_inline_abort_close: bool,
    pub(crate) dispatch_off_reason: Option<&'static str>,
    pub(crate) sinkable_sites_seen: u32,
    pub(crate) accum_bufferable_seen: u32,
    pub(crate) sunk_alloc_seen: u32,
    pub(crate) materialize_emit_count: u32,
    pub(crate) closure_seen: u32,
    pub(crate) inline_kinds: u8,
    pub(crate) inline_depth: u8,
    pub(crate) body_writes: Box<[u32]>,
    pub(crate) downrec_link: Option<(u32, u32)>,
    pub(crate) downrec_multi_way_count: u8,
    pub(crate) tier_up: Option<Box<TierUp>>,
}

/// Moves each named field of a [`CompiledTraceParts`] into a
/// [`CompiledTrace`] with no side trace wired.
macro_rules! moved {
    ($parts:ident => $($f:ident),* $(,)?) => {{
        let CompiledTraceParts { $($f),* } = $parts;
        Self {
            $($f,)*
            side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
            side_children: TRefLock::new(std::collections::HashMap::new()),
            has_any_side_wired: TCellBool::new(false),
        }
    }};
}

impl CompiledTrace {
    /// The trace the dispatcher enters, with no side trace wired yet.
    ///
    /// # Safety
    /// - `parts.entry`, and the optimizing tier's entry its `tier_up` may
    ///   come to hold, is code with the [`TraceFn`] ABI, compiled against
    ///   the entry and exit tags and the window size `parts` states (the
    ///   dispatcher moves registers in and out by them unchecked), that
    ///   lives as long as any Vm the trace is installed on.
    /// - Code whose back edge calls the loop its `tier_up`'s loop cell
    ///   (`__private::tier_up_loop_cell`) holds, when that is not null,
    ///   stores the registers the loop head reads first and returns what
    ///   the loop returns.
    /// - The cells whose addresses that code holds are the ones `parts`
    ///   owns: `exit_side_trace_ptrs`, the cells of the
    ///   `tags_side_trace_ptrs` slice (it is never resized, so they stay
    ///   where they are), `global_side_trace_ptr`, the `side_trace_ptr`
    ///   and `chain` of each of `per_exit_inline`, and the `tier_up` count;
    ///   the side-trace cells are null.
    /// - The `tier_up` source, if any, was made for this trace by the
    ///   backend that compiled it, in the JIT storage of the Vm it is
    ///   installed on.
    pub unsafe fn from_parts(parts: CompiledTraceParts) -> Self {
        moved!(parts => head_pc, entry, n_ops, dispatchable, window_size, exit_tags,
            global_tag_res_kind, entry_tags, per_exit_tags, per_exit_inline, exit_hit_counts,
            exit_side_trace_ptrs, tags_side_trace_ptrs, global_side_trace_ptr,
            is_inline_abort_close, dispatch_off_reason, sinkable_sites_seen,
            accum_bufferable_seen, sunk_alloc_seen, materialize_emit_count, closure_seen,
            inline_kinds, inline_depth, body_writes, downrec_link, downrec_multi_way_count,
            tier_up)
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
