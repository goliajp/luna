//! v2.0 Track J sub-step J-B regression — `Vm.jit.storage` field
//! migration of `JIT_CACHE` / `JIT_CACHE_HANDLES` / `TRACE_JIT_HANDLES`
//! from `thread_local!` to per-`Vm` field storage.
//!
//! What we assert (single-threaded; cross-thread comes in J-D / J-E):
//!
//! 1. Two separate `Vm`s on the same thread maintain SEPARATE JIT
//!    caches. Pre-J-B, both Vms shared the thread-local `JIT_CACHE`;
//!    post-J-B each Vm carries its own `storage.cache`.
//!
//! 2. Within a single `Vm`, the JIT cache still serves second-call
//!    hits (no regression versus the pre-J-B per-thread cache for
//!    intra-`Vm` reuse).
//!
//! 3. Trace JIT compilation parks the trace's `JITModule` on the
//!    `Vm`'s `storage.trace_handles` Vec instead of the deleted
//!    `TRACE_JIT_HANDLES` TLS. Re-evaluating a trace-hot loop on a
//!    second Vm does not error or share trace handles.
//!
//! See `.dev/rfcs/v2.0-track-j-b-design.md` for the migration design.

use luna_core::version::LuaVersion;

/// Two fresh Vms on the same thread cache JIT compiles independently
/// (per-`Vm`, not per-thread).
#[test]
fn two_vms_have_independent_jit_caches() {
    let src = b"local a = 5; local b = 7; return a + b";

    let mut vm_a = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let cl_a = vm_a.load(src, b"=t").expect("vm_a load");
    let r_a = vm_a
        .call_value(luna_core::runtime::Value::Closure(cl_a), &[])
        .expect("vm_a call");
    assert!(matches!(
        r_a.first(),
        Some(luna_core::runtime::Value::Int(12))
    ));
    let n_a = luna_jit::jit_backend::cache_entry_count(&vm_a);
    assert_eq!(n_a, 1, "vm_a cached its compile exactly once");

    // Fresh Vm B; cache is independent from vm_a (pre-J-B both Vms
    // shared the thread-local `JIT_CACHE`).
    let mut vm_b = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let n_b_before = luna_jit::jit_backend::cache_entry_count(&vm_b);
    assert_eq!(
        n_b_before, 0,
        "fresh vm_b starts with an empty per-Vm cache (was non-zero pre-J-B if vm_a had populated the shared TLS)"
    );
    let cl_b = vm_b.load(src, b"=t").expect("vm_b load");
    let r_b = vm_b
        .call_value(luna_core::runtime::Value::Closure(cl_b), &[])
        .expect("vm_b call");
    assert!(matches!(
        r_b.first(),
        Some(luna_core::runtime::Value::Int(12))
    ));
    let n_b_after = luna_jit::jit_backend::cache_entry_count(&vm_b);
    assert_eq!(n_b_after, 1, "vm_b cached its own compile exactly once");

    // vm_a's cache count is unaffected by vm_b's compile activity.
    assert_eq!(
        luna_jit::jit_backend::cache_entry_count(&vm_a),
        1,
        "vm_a cache unchanged by vm_b's compile"
    );
}

/// Within a single Vm, a second call on the same closure hits the
/// per-Vm cache and does not bump the entry count. (Same semantics as
/// pre-J-B; intra-Vm reuse preserved.)
#[test]
fn intra_vm_second_call_hits_cache() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let cl = vm
        .load(b"local a = 5; local b = 7; return a + b", b"=t")
        .expect("compile");

    let v1 = vm
        .call_value(luna_core::runtime::Value::Closure(cl), &[])
        .unwrap();
    let v2 = vm
        .call_value(luna_core::runtime::Value::Closure(cl), &[])
        .unwrap();
    assert!(matches!(
        v1.first(),
        Some(luna_core::runtime::Value::Int(12))
    ));
    assert!(matches!(
        v2.first(),
        Some(luna_core::runtime::Value::Int(12))
    ));
    assert_eq!(
        luna_jit::jit_backend::cache_entry_count(&vm),
        1,
        "second call hits the cache; entry count stays 1"
    );
}

/// Trace JIT: a hot loop compiles a trace on each of two separate Vms
/// and both produce the expected sum without cross-Vm leakage. This
/// exercises `try_compile_trace_with_options` -> `storage.trace_handles`
/// in lieu of the deleted `TRACE_JIT_HANDLES` TLS (no direct counter
/// surface for trace_handles length; the assertion is correctness +
/// no panic on the second Vm's compile).
#[test]
fn trace_jit_works_per_vm_after_storage_migration() {
    let src = "local s = 0 for i = 1, 1000 do s = s + i end return s";

    let mut vm_a = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let r_a = vm_a.eval(src).expect("vm_a eval");
    assert!(matches!(
        r_a.first(),
        Some(&luna_core::runtime::Value::Int(500500))
    ));

    // Second Vm independently compiles and runs.
    let mut vm_b = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let r_b = vm_b.eval(src).expect("vm_b eval");
    assert!(matches!(
        r_b.first(),
        Some(&luna_core::runtime::Value::Int(500500))
    ));

    // Re-run on vm_a to confirm trace handles park survives second use.
    let r_a2 = vm_a.eval(src).expect("vm_a 2nd eval");
    assert!(matches!(
        r_a2.first(),
        Some(&luna_core::runtime::Value::Int(500500))
    ));
}
