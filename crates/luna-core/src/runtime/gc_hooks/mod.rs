//! The hooks of the builds only tests use: `gc-trace` (the accounting
//! trace compared with an instrumented PUC), `gc-verify` (checks of the
//! collector's invariants), `gc-stress` (a collection at every
//! checkpoint). Each hook is empty, or a constant, in the product build;
//! the code that calls them names no feature. What a hook does lives in
//! the modules only those builds compile (`gc_trace`, `heap::trace`,
//! `heap::verify`, ...).

use crate::runtime::heap::GcHeader;

/// The accounting trace is written.
pub(crate) const TRACE: bool = cfg!(feature = "gc-trace");
/// The collector's invariants are checked.
pub(crate) const VERIFY: bool = cfg!(feature = "gc-verify");
/// Every checkpoint collects.
pub(crate) const STRESS: bool = cfg!(feature = "gc-stress");
/// Something watches each checkpoint: it is never only noted.
pub(crate) const CHECKPOINTS_WATCHED: bool = TRACE || VERIFY || STRESS;
/// The checks of debug builds, which gc-verify runs too.
pub(crate) const DEBUG_CHECKS: bool = cfg!(any(debug_assertions, feature = "gc-verify"));

/// What the test-only builds keep on the heap; nothing in the product.
#[derive(Default)]
pub(crate) struct HookState {
    #[cfg(feature = "gc-trace")]
    pub(crate) trace: crate::runtime::gc_trace::Trace,
    /// gc-verify: headers freed since the last collection began, for the
    /// read-time probes (exact only under quarantining allocators)
    #[cfg(feature = "gc-verify")]
    pub(crate) recently_freed: std::collections::HashSet<usize>,
    /// gc-verify: garbage a load set aside that went on the list after
    /// the marking that found it dead (see `Heap::hook_born`), until freed
    #[cfg(feature = "gc-verify")]
    pub(crate) born_dead: std::collections::HashSet<usize>,
}

/// gc-verify: a marking that, instead of marking a child, reports one the
/// sweep about to run would free while the object being traced holds it.
#[derive(Clone, Copy, Default)]
pub(crate) struct ChildCheck {
    #[cfg(feature = "gc-verify")]
    pub(crate) check: Option<(u8, *mut GcHeader, &'static str)>,
}

impl ChildCheck {
    /// Whether this marking only checks.
    #[inline(always)]
    pub(crate) fn active(&self) -> bool {
        #[cfg(feature = "gc-verify")]
        return self.check.is_some();
        #[cfg(not(feature = "gc-verify"))]
        false
    }

    /// The check of child `h`, when active: true when it was checked
    /// rather than marked.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    pub(crate) unsafe fn report(&self, h: *mut GcHeader) -> bool {
        #[cfg(feature = "gc-verify")]
        if let Some(check) = self.check {
            // SAFETY: the caller's contract
            unsafe { crate::runtime::heap::verify::dead_child(h, check) };
            return true;
        }
        let _ = h;
        false
    }
}

/// gc-trace: the lines held back while counting is set aside (see
/// `Heap::defer_begin`); nothing in the product.
#[derive(Default)]
pub(crate) struct HeldLines {
    #[cfg(feature = "gc-trace")]
    lines: Vec<String>,
}

/// gc-trace: the seed the instrumented PUC build takes from the
/// environment too.
#[inline(always)]
pub(crate) fn fixed_seed() -> Option<u32> {
    #[cfg(feature = "gc-trace")]
    return crate::runtime::gc_trace::fixed_seed();
    #[cfg(not(feature = "gc-trace"))]
    None
}

/// gc-trace: the line `luaC_step` logs before (`g`) and after (`G`) a step.
#[inline(always)]
pub(crate) fn hook_step<H: crate::runtime::puc::step::StepHost + ?Sized>(h: &mut H, after: bool) {
    #[cfg(feature = "gc-trace")]
    crate::runtime::puc::trace::trace_step(h, after);
    let _ = (h, after);
}

/// gc-trace: the end of a full collection.
#[inline(always)]
pub(crate) fn hook_full_end<H: crate::runtime::puc::step::StepHost + ?Sized>(h: &mut H) {
    #[cfg(feature = "gc-trace")]
    crate::runtime::puc::trace::trace_full_end(h);
    let _ = h;
}

mod checks;
mod refuse;
mod trace;
