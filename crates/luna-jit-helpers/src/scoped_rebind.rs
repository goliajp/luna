//! RAII rebind of the per-dispatch `JIT_VM` / `JIT_CL` TLS slots.
//!
//! ## Why this exists
//!
//! Simply overwriting the TLS slots in [`super::enter_jit`] and
//! returning a no-op [`luna_core::jit::JitVmGuard`] is only correct
//! under a single-thread, single-level dispatch invariant, where every
//! fresh `enter_jit` overwrites the slots before any helper consults
//! them.
//!
//! `Vm.jit.storage` lets a Vm moved across threads under
//! `feature = "send"` carry its JIT cache + handles with it. The TLS
//! slots that `JIT_VM` / `JIT_CL` synthesize, however, are per-OS-thread — they can't follow a Vm
//! across threads, and they can't naively persist across nested JIT
//! dispatches either (a JIT'd op that calls Lua via a metamethod
//! ends up reentering `enter_jit`; on return the outer entry would
//! be left looking at the inner Vm's slot).
//!
//! This module handles both by:
//!
//! 1. Capturing the prior `(JIT_VM, JIT_CL)` values at every
//!    [`super::enter_jit`] entry.
//! 2. Installing the new values.
//! 3. Restoring the captured prior values from `Drop` on the returned
//!    guard (held by [`super::CraneliftBackend::enter`]).
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
//! Each dispatch pays 2 extra TLS writes on the way out (~5-10
//! cycles each on arm64). On a 434k-dispatch fib_28 run that's
//! ~1.5 ms aggregate. Correctness wins over the elision; if the
//! single-thread fast path is ever a measured bottleneck, a
//! cfg-gated `#[cfg(not(feature = "send"))]` no-op drop variant can
//! be reintroduced.

use luna_core::jit::{JitVmGuard, JitVmRebindRestore};
use luna_core::runtime::{Gc, LuaClosure};
use luna_core::vm::Vm;

use super::{JIT_CL, JIT_VM};

/// Internal restorer used by [`super::enter_jit`]. Writes the captured
/// previous slot values back into the TLS cells. [`JitVmGuard::drop`]
/// calls it once per guard, with the slot values that guard's
/// [`super::enter_jit`] replaced.
fn restore_tls(prev_vm: *mut Vm, prev_cl: *const LuaClosure) {
    JIT_VM.with(|c| c.set(prev_vm));
    JIT_CL.with(|c| c.set(prev_cl));
}

/// Scoped rebind front-door, called by [`super::enter_jit`].
///
/// 1. Snapshots `(JIT_VM, JIT_CL)` into `prev_vm` / `prev_cl`.
/// 2. Installs the dispatcher's new `(vm, cl)` pair.
/// 3. Returns a [`JitVmGuard`] whose drop calls [`restore_tls`] with
///    the snapshot.
///
/// `vm` is only stored, never dereferenced here.
#[inline]
pub(super) fn scoped_jit_vm_rebind(vm: *mut Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
    // 1. Snapshot the previous slots BEFORE the install (otherwise
    //    we'd capture our own newly-installed values).
    let prev_vm = JIT_VM.with(|c| c.get());
    let prev_cl = JIT_CL.with(|c| c.get());

    // 2. Install the new values.
    JIT_VM.with(|c| c.set(vm));
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
