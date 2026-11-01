//! A trace a quicker code generator compiled, on its way to the optimizing
//! tier.

use super::*;

/// A trace on its way to the optimizing tier.
pub struct TierUp {
    /// Loop iterations run in the trace's code plus entries: the code adds
    /// one per iteration through this cell (whose address it holds), the
    /// dispatcher one per entry.
    pub count: Box<TCellU32>,
    /// [`CompileOptions::tier_up_at`] the trace was compiled with.
    pub at: u32,
    /// The head function's `call_hot_count` when the trace was compiled.
    /// Once that function has been called again, the code is being reused
    /// rather than run once, and the trace moves after
    /// `at / TIER_UP_REUSED_DIVISOR` iterations and entries.
    pub calls_at: u32,
    /// The optimizing tier's entry once compiled (null before).
    pub optimized: TCellPtr,
    /// Set once the optimizing tier was asked, whatever it answered.
    pub tried: TCellBool,
    /// For a side trace: the parent's exit cells holding this trace's entry
    /// (addresses of [`TCellPtr`]s; null when not wired).
    pub parent_cells: [TCellPtr; 2],
    /// What the backend compiles the trace from; taken when it does.
    pub source: TRefLock<Option<Box<dyn std::any::Any + Send + Sync>>>,
}
