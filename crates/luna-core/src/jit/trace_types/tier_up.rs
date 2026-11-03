//! A trace a quicker code generator compiled, on its way to the optimizing
//! tier.

use super::*;

/// What the backend compiles a trace from at the optimizing tier.
pub(crate) type Source = Box<dyn std::any::Any + Send + Sync>;

/// A trace on its way to the optimizing tier. Its cells are written only
/// by the dispatcher; the backend reads them and gives the source.
pub struct TierUp {
    /// Loop iterations run in the trace's code plus entries: the code adds
    /// one per iteration through this cell (whose address it holds), the
    /// dispatcher one per entry.
    pub(crate) count: Box<TCellU32>,
    /// [`CompileOptions::tier_up_at`] the trace was compiled with.
    pub(crate) at: u32,
    /// The head function's `call_hot_count` when the trace was compiled.
    /// Once that function has been called again, the code is being reused
    /// rather than run once, and the trace moves after
    /// `at / TIER_UP_REUSED_DIVISOR` iterations and entries.
    pub(crate) calls_at: u32,
    /// The optimizing tier's entry once compiled (null before), for a
    /// trace whose code does not enter it at the loop.
    pub(crate) optimized: TCellPtr,
    /// The optimizing tier's loop once compiled (null before), for a trace
    /// whose code enters it at the loop: the baseline code's back edge
    /// calls it with the loop head's registers in `reg_state`, once its
    /// count reaches `at`.
    pub(crate) looped: TCellPtr,
    /// The optimizing tier's code is entered at the loop (`looped`).
    pub(crate) at_loop: bool,
    /// Set once the optimizing tier was asked, whatever it answered.
    pub(crate) tried: TCellBool,
    /// For a side trace: the parent's exit cells holding this trace's entry
    /// (addresses of [`TCellPtr`]s; null when not wired).
    pub(crate) parent_cells: [TCellPtr; 2],
    /// What the backend compiles the trace from; taken when it does.
    pub(crate) source: TRefLock<Option<Source>>,
}

impl TierUp {
    /// A trace that moves after `at` iterations and entries, counted in
    /// `count` (whose address the trace's code holds), and fewer once its
    /// head function, called `calls_at` times so far, is called again.
    pub fn new(count: Box<TCellU32>, at: u32, calls_at: u32) -> Self {
        Self {
            count,
            at,
            calls_at,
            optimized: TCellPtr::null(),
            looped: TCellPtr::null(),
            at_loop: false,
            tried: TCellBool::new(false),
            parent_cells: [TCellPtr::null(), TCellPtr::null()],
            source: TRefLock::new(None),
        }
    }

    /// The source, while the trace is still the backend's own: a trace's
    /// source can be given only before it is installed (after, see
    /// `__private::put_tier_source`).
    pub fn source_mut(&mut self) -> impl std::ops::DerefMut<Target = Option<Source>> + '_ {
        self.source.borrow_mut()
    }

    /// The cell counting the trace's iterations and entries, whose address
    /// the trace's code holds.
    #[inline]
    pub fn count(&self) -> &TCellU32 {
        &self.count
    }

    /// [`CompileOptions::tier_up_at`] the trace was compiled with.
    #[inline]
    pub fn at(&self) -> u32 {
        self.at
    }
}
