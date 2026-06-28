//! v2.0 J-B follow-up regression — `from_storage` `StorageMismatch`
//! must NOT abort the process.
//!
//! Background: J-B (`e894e90`) moved `JIT_CACHE` / `JIT_CACHE_HANDLES`
//! / `TRACE_JIT_HANDLES` from thread-local!s to `Vm.jit.storage`.
//! The three `from_storage` call sites in `luna_jit::jit_backend`
//! downcast the polymorphic `&mut dyn JitStorage` to the concrete
//! `CraneliftJitStorage`.
//!
//! Pre-fix, the downcast was an `expect` panic. If an embedder
//! installed `CraneliftBackend` without the paired
//! `CraneliftJitStorage` (the trait-pair invariant violation), the
//! first JIT compile attempt panicked. When this happened under a
//! C-ABI callback (any `luaL_*` / `lua_*` entrypoint), the Rust
//! panic crossed an `extern "C"` boundary and the runtime aborted
//! with SIGABRT (`fatal runtime error: failed to initiate panic`).
//!
//! The `capi_zero_result_callback` test in `tests/capi.rs` was the
//! original reproduction — `luaL_newstate` called
//! `install_jit_backend(Cranelift, Cranelift)` without the storage
//! install, so its first JIT compile inside `lua_pcall` aborted the
//! test process. That test was unblocked by routing `luaL_newstate`
//! through `install_default_jit` (which installs both halves), and
//! the deeper defense-in-depth change converted `from_storage` to
//! return `Result<&mut CraneliftJitStorage, StorageMismatch>`.
//!
//! This file is the explicit regression that locks in the
//! defense-in-depth contract: even when an embedder deliberately
//! creates the mismatched configuration `CraneliftBackend +
//! NullJitStorage`, the JIT compile path returns `Skipped` / `None`
//! and the interpreter completes the call. No process abort.
//!
//! Refs:
//! - `.dev/known-bugs/fixed/capi-zero-result-callback-from-storage-downcast-sigabrt.md`
//! - `crates/luna-jit/src/jit_backend/storage.rs` (`from_storage` +
//!   `StorageMismatch`)

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;

/// Build a Vm that has `CraneliftBackend` installed but `NullJitStorage`
/// — the deliberate trait-pair invariant violation that pre-fix caused
/// SIGABRT.
///
/// We don't go through `install_default_jit` / `new_minimal_with_jit`
/// because both correctly install the paired `CraneliftJitStorage`.
/// Instead we mimic the v2.0 J-B-era `luaL_newstate` shape: install the
/// backend only, leave storage at the default `NullJitStorage` (which
/// `Vm::new_minimal` populates via `JitState::with_null_backend()`).
fn vm_with_mismatched_storage() -> luna_core::vm::Vm {
    let mut vm = luna_core::vm::Vm::new_minimal(LuaVersion::Lua55);
    vm.install_jit_backend(
        luna_jit::jit_backend::CraneliftBackend,
        luna_jit::jit_backend::CraneliftBackend,
    );
    // Intentionally NO `install_jit_storage` — that's the bug shape.
    vm
}

/// Sanity: a zero-arg chunk that normally exercises the int-chunk JIT
/// path completes via interp fallback, returning the right value, with
/// no panic / abort.
///
/// Pre-fix: this hit the `cache_lookup_or_compile` → `from_storage`
/// panic on the first compile attempt. Post-fix: `from_storage` returns
/// `Err(StorageMismatch)`, `cache_lookup_or_compile` short-circuits with
/// `None`, the dispatcher takes the standard interp path, and the
/// chunk returns `Int(12)`.
#[test]
fn int_chunk_compile_mismatch_skips_jit_runs_interp() {
    let mut vm = vm_with_mismatched_storage();
    let cl = vm
        .load(b"local a = 5; local b = 7; return a + b", b"=mismatch")
        .expect("compile");
    let r = vm
        .call_value(Value::Closure(cl), &[])
        .expect("call_value returns without panic / abort");
    assert!(
        matches!(r.first(), Some(&Value::Int(12))),
        "result is the interp-computed sum, JIT silently skipped on storage mismatch"
    );
}

/// Trace JIT path: a hot loop normally triggers `try_compile_trace`
/// which parks the trace's `JITModule` on `storage.trace_handles`. With
/// a `NullJitStorage` slot the `from_storage` call returns
/// `Err(StorageMismatch)`, the trace compile returns `None`, the
/// recorder gives up on the trace, and the loop completes under interp.
///
/// Pre-fix: same SIGABRT as the int-chunk path, just triggered at a
/// different `from_storage` site (the one in
/// `jit_backend::trace::try_compile_trace_with_options`).
#[test]
fn trace_compile_mismatch_skips_jit_runs_interp() {
    let mut vm = vm_with_mismatched_storage();
    let r = vm
        .eval("local s = 0 for i = 1, 1000 do s = s + i end return s")
        .expect("eval returns without panic / abort");
    assert!(
        matches!(r.first(), Some(&Value::Int(500500))),
        "sum 1..=1000 = 500500, computed under interp fallback"
    );
}

/// A second compile attempt on the same Vm continues to skip JIT
/// without panicking — the `StorageMismatch` path is reliably
/// idempotent (no torn state between attempts).
#[test]
fn repeated_compile_under_mismatch_remains_stable() {
    let mut vm = vm_with_mismatched_storage();
    for _ in 0..3 {
        let r = vm
            .eval("local a = 1; local b = 2; return a + b")
            .expect("repeated eval succeeds under interp fallback");
        assert!(matches!(r.first(), Some(&Value::Int(3))));
    }
}

/// Direct exercise of the C-ABI path: `luaL_newstate` + `luaL_openlibs`
/// + register a C callback + `lua_pcall` — the exact shape of the
/// original `capi_zero_result_callback` repro. After the `luaL_newstate`
/// fix this no longer reaches the mismatched code path (storage is
/// installed correctly), but we re-assert here that the full C-ABI
/// flow is SIGABRT-free as a guard against regressing
/// `luaL_newstate`'s install-default-jit routing.
#[test]
fn capi_zero_result_callback_no_sigabrt_post_fix() {
    use luna_jit::capi::*;
    use std::ffi::CString;
    extern "C" fn c_void(_: *mut LuaState) -> std::os::raw::c_int {
        0
    }
    let name = CString::new("c_void").unwrap();
    let src = CString::new("return select('#', c_void(1, 2, 3))").unwrap();
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        lua_register(l, name.as_ptr(), c_void);
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(
            lua_pcall(l, 0, 1, 0),
            LUA_OK,
            "pcall completes via interp; pre-fix this aborted with SIGABRT in luaL_newstate's mismatched storage path"
        );
        assert_eq!(lua_tointeger(l, -1), 0);
        lua_close(l);
    }
}
