//! A trace a quicker code generator compiled, on its way to the optimizing
//! tier.

use super::*;

/// What the backend compiles a trace from at the optimizing tier.
pub(crate) type Source = Box<dyn std::any::Any + Send + Sync>;

/// The times a trace at the optimizing tier is compiled again at most
/// (see [`TierUp::retier`]).
pub(crate) const RETIER_CAP: u32 = 4;

/// What a backend hands back with the optimizing tier's code to compile
/// the trace from again (see `TraceCompiler::tier_up_keeping`).
pub struct TierKept {
    /// The source to compile the trace from, made for it by the backend
    /// as the tier source it took.
    pub source: Box<dyn std::any::Any + Send + Sync>,
    /// One of the trace's side cells got a side trace since the backend
    /// read them for the code it hands back, whose exit does not call it:
    /// the move again is due at once.
    pub stale: bool,
}

/// A trace on its way to the optimizing tier. The dispatcher writes its
/// count and entry; the backend reads them, gives the source, and writes
/// the loop and `tried` through `__private::set_tier_up_loop`.
pub struct TierUp {
    /// Loop iterations run in the trace's code plus entries: the code adds
    /// one per iteration through this cell (whose address it holds), the
    /// dispatcher one per entry.
    pub(crate) count: Box<TCellU32>,
    /// [`CompileOptions::tier_up_at`] the trace was compiled with.
    pub(crate) at: u32,
    /// The head function's `call_hot_count` when the trace was compiled.
    /// Once that function has been called again, the code is being reused
    /// rather than run once, and the trace moves after `reused_at`
    /// iterations and entries.
    pub(crate) calls_at: u32,
    /// `at / TIER_UP_REUSED_DIVISOR`, or `at` for a trace whose `at` is
    /// where the move pays for itself however the code is reused (see
    /// [`TierUp::set_payback_at`]).
    pub(crate) reused_at: u32,
    /// The optimizing tier's entry once compiled (null before).
    pub(crate) optimized: TCellPtr,
    /// The optimizing tier's loop once compiled (null before), for a trace
    /// whose code calls it from its back edge with the loop head's
    /// registers in `reg_state` (see `__private::set_tier_up_loop`).
    pub(crate) looped: TCellPtr,
    /// Set once the optimizing tier was asked, whatever it answered.
    pub(crate) tried: TCellBool,
    /// What the backend compiles the trace from; taken when it does.
    pub(crate) source: TRefLock<Option<Source>>,
    /// What the backend compiles the trace from again once at the
    /// optimizing tier, moved to `source` by [`TierUp::retier`].
    pub(crate) kept: TRefLock<Option<Source>>,
    /// The moves again asked for so far.
    pub(crate) retiers: TCellU32,
    /// The move asked for is a move again, until the backend is done.
    pub(crate) retiering: TCellBool,
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
            reused_at: at / TIER_UP_REUSED_DIVISOR,
            optimized: TCellPtr::null(),
            looped: TCellPtr::null(),
            tried: TCellBool::new(false),
            source: TRefLock::new(None),
            kept: TRefLock::new(None),
            retiers: TCellU32::new(0),
            retiering: TCellBool::new(false),
        }
    }

    /// The source, while the trace is still the backend's own: a trace's
    /// source can be given only before it is installed (after, see
    /// `__private::put_tier_source`).
    pub fn source_mut(&mut self) -> impl std::ops::DerefMut<Target = Option<Source>> + '_ {
        self.source.borrow_mut()
    }

    /// What the backend compiles the trace from again once at the
    /// optimizing tier, while the trace is still the backend's own (see
    /// [`TierUp::source_mut`]): for code taken from another Vm that is
    /// already the optimizing tier's.
    pub fn kept_mut(&mut self) -> impl std::ops::DerefMut<Target = Option<Source>> + '_ {
        self.kept.borrow_mut()
    }

    /// Asks for the move again at the next entry, from the source kept,
    /// when there is one and fewer than [`RETIER_CAP`] moves again were
    /// asked for: one of the trace's side cells just got a side trace, which
    /// the optimizing tier's code calls only once compiled again.
    pub(crate) fn retier(&self) {
        if self.retiers.get() >= RETIER_CAP {
            return;
        }
        let Some(s) = self.kept.borrow_mut().take() else {
            return;
        };
        *self.source.borrow_mut() = Some(s);
        self.retiers.set(self.retiers.get() + 1);
        self.retiering.set(true);
        self.tried.set(false);
        self.count.set(self.at.saturating_sub(1));
    }

    /// The cell counting the trace's iterations and entries, whose address
    /// the trace's code holds.
    #[inline]
    pub fn count(&self) -> &TCellU32 {
        &self.count
    }

    /// [`CompileOptions::tier_up_at`] the trace was compiled with, or the
    /// count [`TierUp::set_payback_at`] set.
    #[inline]
    pub fn at(&self) -> u32 {
        self.at
    }

    /// [`TierUp::reused_at`]: where the trace moves once its head function
    /// has been called again.
    #[inline]
    pub fn reused_at(&self) -> u32 {
        self.reused_at
    }

    /// Has the trace move after `at` iterations and entries whether or not
    /// its head function is called again: `at` being where the cost of the
    /// move is earned back, reuse is already weighed in it.
    pub fn set_payback_at(&mut self, at: u32) {
        (self.at, self.reused_at) = (at, at);
    }
}
