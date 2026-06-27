//! v2.0 Track J sub-step J-D — RAII rebind of the per-dispatch
//! `JIT_VM` / `JIT_CL` TLS slots.
//!
//! ## Why this exists
//!
//! Prior to J-D, [`super::enter_jit`] simply overwrote the TLS slots
//! and returned a [`luna_core::jit::JitVmGuard`] whose drop was a
//! no-op. That worked under the single-thread, single-level dispatch
//! invariant: every fresh `enter_jit` overwrites the slots before any
//! helper consults them, so stale values left after a previous
//! dispatch were harmless.
//!
//! J-B installed `Vm.jit.storage` so cross-thread Vm move under
//! `feature = "send"` (the J-E flip) can park its JIT cache + handles
//! with the Vm itself. The TLS slots that `JIT_VM` / `JIT_CL`
//! synthesize, however, are per-OS-thread — they can't follow a Vm
//! across threads, and they can't naively persist across nested JIT
//! dispatches either (a JIT'd op that calls Lua via a metamethod
//! ends up reentering `enter_jit`; on return the outer entry would
//! be left looking at the inner Vm's slot).
//!
//! J-D fixes both by:
//!
//! 1. Capturing the prior `(JIT_VM, JIT_CL)` values at every
//!    [`super::enter_jit`] entry (Phase B of the sub-step).
//! 2. Installing the new values exactly as before.
//! 3. Restoring the captured prior values from `Drop` on the returned
//!    guard (Phase C wires the guard into [`super::CraneliftBackend::enter`]).
//!
//! ## Nesting semantics
//!
//! The capture-on-enter / restore-on-drop pattern is intrinsically
//! LIFO-safe: nested `enter_jit` calls each carry their own captured
//! parent state, and unwinding pops them in the correct order. No
//! depth counter is needed; the call-stack discipline of the
//! dispatcher is the depth tracker.
//!
//! ## Single-thread perf cost
//!
//! Each dispatch now pays 2 extra TLS writes on the way out (~5-10
//! cycles each on arm64). On a 434k-dispatch fib_28 run that's
//! ~1.5 ms aggregate. Correctness wins over the elision; if the
//! single-thread fast path is ever a measured bottleneck again, a
//! cfg-gated `#[cfg(not(feature = "send"))]` no-op drop variant can
//! be reintroduced as a `J-E perf polish` follow-up — see
//! `.dev/rfcs/v2.0-track-j-d-verdict.md` §"J-E handoff".

use luna_core::jit::{JitVmGuard, JitVmRebindRestore};
use luna_core::runtime::{Gc, LuaClosure};
use luna_core::vm::Vm;

use super::{JIT_CL, JIT_VM};

/// Internal restorer used by [`super::enter_jit`]. Writes the captured
/// previous slot values back into the TLS cells.
///
/// SAFETY: the function pointer is wrapped in `JitVmRebindRestore` and
/// only ever invoked via [`JitVmGuard::drop`], which calls it once per
/// guard at scope end. The cells are plain `Cell<*mut Vm>` /
/// `Cell<*const LuaClosure>` so the write itself is not unsafe Rust;
/// the `unsafe fn` signature carries the contract that the slot
/// participates in the JIT helper invariant (`current_jit_vm` /
/// `current_jit_closure` debug-assert non-null).
pub(super) unsafe fn restore_tls(prev_vm: *mut Vm, prev_cl: *const LuaClosure) {
    JIT_VM.with(|c| c.set(prev_vm));
    JIT_CL.with(|c| c.set(prev_cl));
}

/// J-D's scoped rebind front-door, called by [`super::enter_jit`].
///
/// 1. Snapshots `(JIT_VM, JIT_CL)` into `prev_vm` / `prev_cl`.
/// 2. Installs the dispatcher's new `(vm, cl)` pair.
/// 3. Returns a [`JitVmGuard`] whose drop calls [`restore_tls`] with
///    the snapshot.
#[inline]
pub(super) fn scoped_jit_vm_rebind(vm: &mut Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
    // 1. Snapshot the previous slots BEFORE the install (otherwise
    //    we'd capture our own newly-installed values).
    let prev_vm = JIT_VM.with(|c| c.get());
    let prev_cl = JIT_CL.with(|c| c.get());

    // 2. Install the new values.
    JIT_VM.with(|c| c.set(vm as *mut Vm));
    let cl_ptr = cl
        .map(|c| c.as_ptr() as *const LuaClosure)
        .unwrap_or(std::ptr::null());
    JIT_CL.with(|c| c.set(cl_ptr));

    // 3. Build the guard with restore hook pointing at `restore_tls`.
    JitVmGuard::from_restore(JitVmRebindRestore {
        prev_vm,
        prev_cl,
        restore_fn: restore_tls,
    })
}
