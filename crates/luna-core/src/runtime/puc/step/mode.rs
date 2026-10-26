//! PUC `luaC_changemode`: the collector switching between incremental
//! and generational mode.

use super::*;

/// The collector goes from mode `prev` to `new` ("incremental" or
/// "generational"): 5.4 and 5.5 enter generational mode with a full
/// marking and sweep (`entergen`), which runs no finalizer; 5.2 runs its
/// cycle on to the propagation; going back, 5.5 sweeps first.
pub(crate) fn change_mode<H: StepHost + ?Sized>(h: &mut H, prev: &str, new: &str) {
    let d = h.heap().puc.dialect;
    if new == "generational" && prev != new && d >= Dialect::L54 {
        h.heap().trace_line(format_args!("X"));
        if !h.finalizing() && !h.heap().no_gc() {
            h.clear_dead_stack();
            full_collect(h, None);
            h.heap().finalizers_due();
            gc_full_pause(h);
            gc_minor_debt(h);
        }
        h.heap().trace_line(format_args!("x"));
    }
    if prev != new && d == Dialect::L52 {
        match new {
            "generational" => enter_generational_52(h),
            _ => leave_generational_52(h),
        }
    }
    if new == "incremental" && prev == "generational" {
        gc_leave_generational(h);
    }
}

impl Heap {
    /// Whether the standalone interpreter restarts the collector in
    /// generational mode before it runs anything (5.4 on `lua.c`).
    pub(in crate::runtime) fn lua_c_starts_generational(&self) -> bool {
        self.puc.dialect >= Dialect::L54
    }
}
