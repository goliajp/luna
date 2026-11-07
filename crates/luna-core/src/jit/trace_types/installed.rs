//! A compiled trace as the dispatcher enters it.

use super::tier_up::TierUp;
use super::*;

pub(crate) mod entry_plan;
pub(crate) mod exit_plan;
pub(crate) mod plans;
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
///     ct.side_children.borrow_mut().fill(None); // error[E0616]: field `side_children` is private
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
    /// The tags and plans of [`CompiledTraceParts::plans`], read in place:
    /// a dispatch takes no count of it.
    pub(crate) plans: std::sync::Arc<plans::Plans>,
    /// A copy of the plans' entry plan, read by every dispatch without
    /// going through `plans`.
    pub(crate) entry_plan: entry_plan::EntryPlan,
    pub(crate) per_exit_inline: TArc<[InlineSideExit]>,
    pub(crate) exit_hit_counts: TArc<[TCellU32]>,
    /// The side trace wired to each of this trace's exits, indexed as
    /// `exit_hit_counts`: an exit that runs one finds it without hashing.
    /// Held here rather than looked up on a proto: a side trace started at
    /// an exit inside a function the trace inlined is cached on that
    /// function's proto.
    pub(crate) side_children: TRefLock<Box<[Option<TArc<CompiledTrace>>]>>,
    /// Fast-path short-circuit hint for the
    /// dispatcher's tentative-decode + cell-load + check path. Set
    /// to `true` by the close handler when a side trace is first wired
    /// to any of this trace's exits (it compiled and the shape gate
    /// passed). Stays `true` for the trace's lifetime — once any
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
    /// See [`CompiledTraceParts::side_cells`].
    pub(crate) side_cells: super::side_cell::SideCells,
    /// See [`CompiledTraceParts::link_pc`].
    pub(crate) link_pc: Option<u32>,
    /// See [`CompiledTraceParts::loop_tags`].
    pub(crate) loop_tags: Option<Box<[ExitTag]>>,
    /// The code counts in this call's word of `reg_state`, not in
    /// `tier_up`'s cell: it has a `tier_up` and its plans count
    /// [`plans::PassCount::PerCall`] (see `frame::trace_count_slot`).
    pub(crate) call_count: bool,
}

/// Moves each named field of a [`CompiledTraceParts`] into a
/// [`CompiledTrace`], with no room for side traces yet.
macro_rules! moved {
    ($parts:ident => $($f:ident),* $(,)?) => {{
        let CompiledTraceParts { $($f,)* plans } = $parts;
        Self {
            call_count: false,
            side_children: TRefLock::new(Box::default()),
            entry_plan: plans.0.entry_plan.clone(),
            plans: plans.0,
            $($f,)*
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
    /// - `parts.plans` is of that compile: made by [`plans::TracePlans::new`]
    ///   from the tags the code was compiled against, its inline exits in
    ///   the order of `per_exit_inline`, or a clone of the plans another
    ///   trace was made from by the same compile: an exit's plan, worked out
    ///   once, serves every trace sharing them. Debug builds check the
    ///   lengths, the number of inline exits and their resume pcs.
    /// - Code that counts its passes in this call's words past the window
    ///   (`frame::trace_count_slot`) has plans made with
    ///   [`plans::PassCount::PerCall`], and a `tier_up`; code with plans
    ///   made with [`plans::PassCount::Shared`] counts in `tier_up`'s count
    ///   cell, if anywhere. Debug builds check the `tier_up`.
    /// - Code whose back edge calls the loop its `tier_up`'s loop cell
    ///   (`__private::tier_up_loop_cell`) holds, when that is not null,
    ///   stores the registers the loop head reads first and returns what
    ///   the loop returns.
    /// - The cells whose addresses that code holds are the ones `parts`
    ///   owns: the `chain` of each of `per_exit_inline`, the count and the
    ///   loop cell of `tier_up`, and `side_cells`.
    /// - Code that reads a side cell calls the entry it holds, when that is
    ///   not null, at the typed exit the cell belongs to, with the exit's
    ///   registers in `reg_state`; it goes on at its loop's start when that
    ///   returns with [`crate::__private::EXIT_RELINK`], and otherwise
    ///   returns what it returned, noting the exit in the word past the
    ///   window (see `frame::trace_ran_side_slot`).
    /// - The `tier_up` source, if any, was made for this trace by the
    ///   backend that compiled it, in the JIT storage of the Vm it is
    ///   installed on.
    pub unsafe fn from_parts(parts: CompiledTraceParts) -> Self {
        let p = &parts.plans.0;
        debug_assert!(
            p.entry_tags.len() <= parts.window_size as usize
                && p.exit_tags.len() <= parts.window_size as usize
                && p.inline_tags.len() == parts.per_exit_inline.len()
                && (parts.per_exit_inline.iter().enumerate())
                    .all(|(i, e)| p.inline_tags.cont_pc(i) == e.cont_pc),
            "the trace's plans are not of its code: {:?}, window {}, {} inline exits",
            parts.plans,
            parts.window_size,
            parts.per_exit_inline.len()
        );
        let per_call = matches!(p.pass_count, plans::PassCount::PerCall(_));
        debug_assert!(
            !per_call || parts.tier_up.is_some(),
            "code counting per call has no tier-up count"
        );
        let call_count = per_call && parts.tier_up.is_some();
        let ct = moved!(parts => head_pc, entry, n_ops, dispatchable, window_size,
            per_exit_inline, exit_hit_counts,
            is_inline_abort_close, dispatch_off_reason, sinkable_sites_seen,
            accum_bufferable_seen, sunk_alloc_seen, materialize_emit_count, closure_seen,
            inline_kinds, inline_depth, body_writes, downrec_link, downrec_multi_way_count,
            tier_up, side_cells, link_pc, loop_tags);
        let side_children = TRefLock::new(vec![None; ct.exit_hit_counts.len()].into_boxed_slice());
        Self {
            call_count,
            side_children,
            ..ct
        }
    }

    /// The times side traces its code ran went on in its loop, over its
    /// side cells (each count wrapping at `u32::MAX`).
    pub fn side_relinks(&self) -> u64 {
        self.side_cells
            .iter()
            .map(|(_, c)| u64::from(c.relinks()))
            .sum()
    }

    /// Per side cell: whether it holds a side trace's entry, and whether
    /// that is the side trace's optimizing tier's.
    pub fn side_cell_states(&self) -> Vec<(bool, bool)> {
        let held = self.side_children.borrow();
        (self.side_cells.iter())
            .map(|(e, c)| {
                let child = held.get(self.per_exit_inline.len() + *e as usize);
                let optimized = child.cloned().flatten().and_then(|ch| {
                    let o = ch.tier_up.as_ref()?.optimized.get();
                    (!o.is_null()).then_some(o)
                });
                let entry = c.entry.get();
                (!entry.is_null(), optimized.is_some_and(|o| o == entry))
            })
            .collect()
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
            .field("exit_tags", &self.plans.exit_tags)
            .field("entry", &"<fn>")
            .finish()
    }
}
