//! `Op::GetUpval` whitelist + emit.
//!
//! fib's body fetches the recursive callee via `GetUpval(0)` to
//! reach the local `f` (captured as upval inside the closure
//! literal). Without this op in the trace JIT whitelist, fib's
//! recorded trace fails compile validation at the first GetUpval.
//! The trace JIT has:
//!
//! - `Op::GetUpval` in `is_whitelisted_op`
//! - `luna_jit_upval_get` symbol registration + sig declaration in
//!   the lowerer's JITModule setup
//! - body emit: `call upval_get(B)` → store raw payload to R[A]

use luna_jit::version::LuaVersion;

#[test]
fn fib_trace_compiles_after_getupval_whitelist() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(12)",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(144)));

    assert!(
        vm.trace_closed_count() >= 1,
        "fib's trace must close (close on call re-entry); got closed={}",
        vm.trace_closed_count()
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "fib's trace must compile (GetUpval is whitelisted); \
         got compiled={} failed={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count()
    );
    // fib's trace dispatches via the inline self-rec emit path. The
    // MIN_DISPATCHABLE_TRUNC_BODY length-gate is skipped when
    // `per_exit_inline` is non-empty (inline traces are
    // dispatch-productive regardless of body length — one dispatch
    // tears through multiple recursion levels via the frame-mat
    // helper).
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib's trace must dispatch via the inline emit path; \
         got dispatched={}",
        vm.trace_dispatched_count()
    );
}

/// All-Lua-dialect smoke: GetUpval whitelist doesn't break the
/// existing pre-5.3 / 5.3 / 5.4 / 5.5 trace_jit pipelines.
#[test]
fn fib_compiles_under_all_dialects() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(true);
        let r = vm
            .eval(
                "local function f(n)
                     if n < 2 then return n end
                     return f(n-1) + f(n-2)
                 end
                 return f(10)",
            )
            .unwrap();
        let n = match r[0] {
            luna_jit::runtime::Value::Int(n) => n,
            luna_jit::runtime::Value::Float(f) => f as i64,
            _ => panic!("expected number, got {:?}", r[0]),
        };
        assert_eq!(n, 55, "fib(10) wrong under {v:?}");
    }
}
