//! Shared `luna_jit_*` extern-C runtime helpers and the per-thread
//! `JIT_VM` / `JIT_CL` TLS slots plus the `enter_jit` RAII rebind.
//! Both `luna-jit` (Cranelift) and `luna-jit-llvm` (alt backend)
//! use this crate so they share one symbol table and one TLS
//! discipline.
//!
//! luna-jit re-exports everything in this crate via
//! `pub use luna_jit_helpers::*;` from `jit_backend/mod.rs`, so all
//! `crate::jit_backend::luna_jit_*` / `super::luna_jit_*`
//! paths resolve unchanged. luna-jit-llvm depends on this crate
//! directly without pulling Cranelift.
//!
//!
//! # Invariants
//!
//! - Symbol names are `#[unsafe(no_mangle)] pub unsafe extern "C" fn
//!   luna_jit_*` — Cranelift's `Linkage::Import` resolves them by
//!   linker symbol; LLVM's `Module::add_function` resolves them via
//!   JIT execution-engine `add_global_mapping`.
//! - Every helper is called only under an active `enter_jit` guard
//!   (which pins `JIT_VM` / `JIT_CL` for the dispatch window) and
//!   reads the Vm/closure pointer via `current_jit_vm()` /
//!   `current_jit_closure()`. Each helper's `# Safety` section names
//!   what else its arguments must be.

// All helpers use fully-qualified `luna_core::*` paths internally.
// Only the `JitVmGuard` re-export is needed by the `enter_jit`
// signature below.
use luna_core::jit::JitVmGuard;

thread_local! {
    /// Current `Vm` pointer for Rust helpers called from
    /// JIT'd code. Set by [`enter_jit`] just before invoking the
    /// entry fn; cleared (RAII via [`JitVmGuard`]) on return. Helpers
    /// (`luna_jit_new_table`, `luna_jit_table_set_int`, etc.) read
    /// this to reach `Vm.heap`. Null when no JIT call is in flight.
    static JIT_VM: std::cell::Cell<*mut luna_core::vm::Vm> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
    /// Current `LuaClosure` pointer for `Op::GetUpval`
    /// value-read helpers. Set alongside `JIT_VM` by [`enter_jit`].
    /// Null when no JIT call is in flight, or when the active call
    /// has no upvalues (zero-upval Protos never reach
    /// `luna_jit_upval_get`).
    static JIT_CL: std::cell::Cell<*const luna_core::runtime::LuaClosure> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

/// Install `vm` as the current JIT Vm pointer. Returns a
/// [`JitVmGuard`] whose drop restores the prior `(JIT_VM, JIT_CL)`
/// values. Must be held across the JIT entry-fn
/// call so any helper can pick the pointer up.
///
/// The guard type itself lives in `luna_core::jit` so the trait
/// signature in `IntChunkCompiler::enter` doesn't drag Cranelift into
/// luna-core.
///
/// # Capture-and-restore
///
/// Overwriting the TLS slots and returning an inert guard is only
/// safe under single-thread, single-level dispatch. Cross-thread Vm
/// move plus nested JIT entry (e.g. JIT'd op → metamethod →
/// Lua-from-Rust → JIT entry again) makes a no-op drop unsafe: the
/// outer entry would resume holding the inner Vm's slot. This
/// therefore delegates to a crate-private
/// `scoped_rebind::scoped_jit_vm_rebind`, which snapshots the
/// previous values into the guard and restores them on drop.
///
/// The `cl` parameter is the closure being invoked. The
/// guard also pins it in `JIT_CL` so `luna_jit_upval_get` can fetch
/// `cl.upvals[idx]` at runtime. Callers that don't need upvalues (the
/// zero-arg host-call path before `Op::GetUpval` was JIT'd) can pass
/// `None`; helpers will hit the debug-assert if they fire.
pub fn enter_jit(
    vm: &mut luna_core::vm::Vm,
    cl: Option<luna_core::runtime::Gc<luna_core::runtime::LuaClosure>>,
) -> JitVmGuard {
    scoped_rebind::scoped_jit_vm_rebind(vm, cl)
}

/// Test-only inspector of the active `(JIT_VM, JIT_CL)` TLS
/// pointers. Used by the scoped-rebind regression test
/// (`luna-jit/tests/it/jit_vm_scoped_rebind.rs`) to assert RAII install +
/// restore semantics across nested [`enter_jit`] calls. Not part of
/// the embedder API.
#[doc(hidden)]
pub fn __jit_tls_ptrs() -> (
    *mut luna_core::vm::Vm,
    *const luna_core::runtime::LuaClosure,
) {
    let vm = JIT_VM.with(|c| c.get());
    let cl = JIT_CL.with(|c| c.get());
    (vm, cl)
}

/// The Vm that entered the running compiled code.
///
/// # Safety
/// The caller runs inside an [`enter_jit`] window on this thread, and
/// does not hold the returned borrow past its own return.
#[inline]
unsafe fn current_jit_vm<'a>() -> &'a mut luna_core::vm::Vm {
    let p = JIT_VM.with(|cell| cell.get());
    debug_assert!(!p.is_null(), "JIT helper called outside enter_jit scope");
    // SAFETY: inside an enter_jit window JIT_VM holds the `&mut Vm` the
    // dispatcher passed to `enter_jit`; the dispatcher does not touch that
    // Vm until the compiled code (and so this helper) returns, so this is
    // the only live borrow of it
    unsafe { &mut *p }
}

/// The closure the running compiled code was entered with.
///
/// # Safety
/// The caller runs inside an [`enter_jit`] window on this thread that
/// was opened with `Some(closure)`.
#[inline]
unsafe fn current_jit_closure() -> luna_core::runtime::Gc<luna_core::runtime::LuaClosure> {
    let p = JIT_CL.with(|cell| cell.get());
    debug_assert!(
        !p.is_null(),
        "luna_jit_upval_get called outside an upval-aware enter_jit scope"
    );
    // SAFETY: inside a window opened with `Some(closure)` JIT_CL holds that
    // closure, which the dispatcher keeps alive until the window closes
    unsafe { luna_core::runtime::Gc::from_ptr(p as *mut luna_core::runtime::LuaClosure) }
}

/// The table whose pointer compiled code passed as a register payload.
///
/// # Safety
/// `p` is the pointer of a live table.
#[inline(always)]
unsafe fn table_arg(p: i64) -> luna_core::runtime::Gc<luna_core::runtime::Table> {
    // SAFETY: the caller's contract
    unsafe { luna_core::runtime::Gc::from_ptr(p as *mut luna_core::runtime::Table) }
}

/// The string whose pointer compiled code passed as a register payload or
/// baked in as a constant.
///
/// # Safety
/// `p` is the pointer of a live string.
#[inline(always)]
unsafe fn str_arg(p: i64) -> luna_core::runtime::Gc<luna_core::runtime::LuaStr> {
    // SAFETY: the caller's contract
    unsafe { luna_core::runtime::Gc::from_ptr(p as *mut luna_core::runtime::LuaStr) }
}

/// The payload word of `v` as compiled code keeps a register: the
/// integer, the float's bits, or the object pointer; zero for nil and
/// the booleans.
#[inline]
fn payload_bits(v: luna_core::runtime::Value) -> i64 {
    raw_bits(v.unpack().1)
}

/// `raw`'s eight bytes as an integer.
#[inline]
fn raw_bits(raw: luna_core::runtime::value::RawVal) -> i64 {
    // SAFETY: every `RawVal` field is eight bytes wide here (asserted
    // below), so whichever field the value was written through, all of
    // `zero` is initialised
    unsafe { raw.zero as i64 }
}

// `raw_bits` reads a pointer payload as `u64`; the JIT backends only
// target 64-bit hosts
const _: () = assert!(std::mem::size_of::<*const ()>() == 8);

mod table_write;
pub use table_write::*;
mod table_read;
pub use table_read::*;
mod upval;
pub use upval::*;
mod guards;
pub use guards::*;
mod stack_ops;
pub use stack_ops::*;
mod materialize;
pub use materialize::*;

mod str_buf;
pub use str_buf::*;

// scoped_rebind submodule (formerly luna-jit/src/jit_backend/scoped_rebind.rs).
mod scoped_rebind;
