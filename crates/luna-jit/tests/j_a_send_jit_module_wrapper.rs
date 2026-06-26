//! v2.0 Track J sub-step J-A — `SendJitModule` wrapper regression.
//!
//! Two guards:
//!
//! 1. **Static assertion** — `SendJitModule: Send`. If a future
//!    cranelift bump introduces a new `!Send` field that isn't
//!    `unsafe impl Send` covered (e.g. the audit Sub 3 caveat in
//!    `.dev/rfcs/v2.0-track-j-prep.md`), this test fails to compile
//!    and becomes the J-A canary.
//!
//! 2. **Cross-thread smoke** — wrap a freshly-built `JITModule`
//!    (default memory provider, which is `Send` per audit Sub 4),
//!    move it across a `std::thread::spawn` boundary, and round-trip
//!    it back. No JIT compilation is exercised — the test only
//!    verifies the `Send` transfer compiles + runs, which is the
//!    J-A wrapper's whole responsibility.
//!
//! The full per-Vm JIT cache cross-thread smoke (with JIT'd code
//! dispatched on the worker thread) lands later as
//! `cv_send_vm_jit_smoke.rs` once J-B / J-D complete (see J prep doc
//! §Sub 5).

use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::Module;
use luna_jit::jit::__SendJitModule_for_j_a_test as SendJitModule;

/// Compile-time assertion: `SendJitModule` is `Send`.
///
/// `fn require_send<T: Send>()` per the J-A task spec. Used both as
/// a `const` enforcement and as a runtime no-op in the smoke test.
const fn require_send<T: Send>() {}

#[test]
fn send_jit_module_static_assert_send() {
    // Will fail to compile if `SendJitModule: !Send`.
    require_send::<SendJitModule>();
}

#[test]
fn send_jit_module_crosses_thread_boundary() {
    require_send::<SendJitModule>();

    // Build a `JITModule` via the default path. The J-A SAFETY
    // contract gates on this — `JITBuilder::default()` /
    // `JITBuilder::new` resolves the memory provider to
    // `SystemMemoryProvider`, which IS `Send`.
    let builder = JITBuilder::new(cranelift_module::default_libcall_names())
        .expect("JITBuilder default isa available on host");
    let module = JITModule::new(builder);
    let wrapped = SendJitModule::new(module);

    // Forward leg: hand the wrapper to a worker thread.
    let handle = std::thread::spawn(move || {
        // Read-only access to the inner module from the worker.
        // Using only operations that don't mutate the module — we're
        // verifying the Send transfer, not the J-B mutator path.
        let _isa_endianness = wrapped.isa().endianness();
        wrapped
    });
    let wrapped = handle.join().expect("worker thread panicked");

    // Return leg: round-trip back.
    let handle = std::thread::spawn(move || wrapped);
    let wrapped = handle.join().expect("return-leg worker panicked");

    // `into_inner()` discards the Send marker. The wrapped module
    // must still be valid (J-A wrapper is a zero-cost newtype).
    let _module: JITModule = wrapped.into_inner();
}
