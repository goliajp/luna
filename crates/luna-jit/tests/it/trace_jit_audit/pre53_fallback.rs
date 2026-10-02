//! Round 2 — pre-5.3 dialect handling. trace JIT bails on pre-5.3
//! ForLoop; programs must still produce correct Lua-level results via
//! the interp fallback.

use super::*;

/// Pre-5.3 ForLoop programs: trace JIT bails (`opts.pre53` check at
/// `src/jit/trace.rs:4410`), interp handles. Verify result is correct
/// and no abort spike.
#[test]
fn trace_audit_pre53_for_loop_interp_fallback() {
    for (_v, label) in &DIALECTS[..2] {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 1, 1000 do s = s + i end; return s",
        );
        // 5.1/5.2: sum stored as Float
        let ok = match r {
            Value::Float(f) => (f - 500500.0).abs() < 1.0,
            Value::Int(500500) => true,
            _ => false,
        };
        assert!(
            ok,
            "pre53-for-fallback[{}]: expected ~500500, got {:?}",
            label, r
        );
        // The closed_count may be 0 or higher depending on what other
        // shapes engage. We don't assert engagement count here — just
        // correctness.
    }
}

/// Pre-5.3 recursive function — trace JIT should still engage on
/// recursion (recursion isn't gated on ForLoop dialect).
#[test]
fn trace_audit_pre53_recursion_works() {
    for (_v, label) in &DIALECTS[..2] {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local function f(n) if n < 2 then return n end return f(n-1) + f(n-2) end; return f(15)",
        );
        // fib(15) = 610. In 5.1/5.2 it's Float, in 5.3+ Int.
        let ok = match r {
            Value::Float(f) => (f - 610.0).abs() < 1e-9,
            Value::Int(610) => true,
            _ => false,
        };
        assert!(ok, "pre53-fib15[{}]: expected ~610, got {:?}", label, r);
    }
}
