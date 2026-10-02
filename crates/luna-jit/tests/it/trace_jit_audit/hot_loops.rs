//! Round 1 — basic hot-loop traces compile + dispatch correctly.

use super::*;

/// Hot loop with simple int sum — most common shape. Trace JIT
/// doesn't engage on flat loops (only specific shapes like recursion
/// + body-style loops), so trace_compiled_count may stay 0. The
/// test exists to verify the result is CORRECT under trace-only Vm
/// where any trace that compiles must produce correct output.
#[test]
fn trace_audit_hot_int_sum() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 1, 10000 do s = s + i end; return s",
        );
        let ok = match r {
            Value::Int(n) => n == 50_005_000,
            _ => false,
        };
        assert!(ok, "hot-int-sum[{}]: expected 50005000, got {:?}", label, r);
    }
}

/// Hot loop with float sum.
#[test]
fn trace_audit_hot_float_sum() {
    for (_v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local s = 0.0; for i = 1, 1000 do s = s + 1.5 end; return s",
        );
        let ok = match r {
            Value::Float(f) => (f - 1500.0).abs() < 1e-9,
            _ => false,
        };
        assert!(
            ok,
            "hot-float-sum[{}]: expected ~1500.0, got {:?}",
            label, r
        );
    }
}

/// Self-recursive fib trace — the canonical trace JIT shape.
#[test]
fn trace_audit_fib_recursive() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local function f(n) if n < 2 then return n end return f(n-1) + f(n-2) end; return f(20)",
        );
        // fib(20) = 6765
        assert!(
            matches!(r, Value::Int(6765)),
            "fib20[{}]: expected Int(6765), got {:?}",
            label,
            r
        );
    }
}

/// Hot loop with table reads (engages JIT table fast path).
#[test]
fn trace_audit_hot_table_read() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}
             for i = 1, 100 do t[i] = i * 2 end
             local s = 0
             for i = 1, 100 do s = s + t[i] end
             return s",
        );
        // sum(2, 4, ..., 200) = 2 * 5050 = 10100
        assert!(
            matches!(r, Value::Int(10100)),
            "hot-tbl-read[{}]: expected Int(10100), got {:?}",
            label,
            r
        );
    }
}

/// Multiple traces from same Proto (different paths through the same
/// function get recorded separately).
#[test]
fn trace_audit_branching_path() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 1000 do
                 if i % 2 == 0 then s = s + i
                 else s = s - i end
             end
             return s",
        );
        // sum of evens (2..1000) - sum of odds (1..999) = 500 (every pair contributes +1)
        // even sum: 250500, odd sum: 250000, diff: 500
        assert!(
            matches!(r, Value::Int(500)),
            "branch-path[{}]: expected Int(500), got {:?}",
            label,
            r
        );
    }
}

/// Nested loops — outer loop body itself is a hot path.
/// FIXED at src/jit/trace.rs ForLoop continue branch: cont_pc now uses
/// (rop.pc + 1) - bx (the body start) instead of record.head_pc, so a
/// side-trace whose head_pc lands on the ForLoop op itself (not the
/// back-edge target) no longer double-advances the outer counter via
/// "trace + interp both run outer ForLoop". See
/// docs/known-bugs/fixed/trace-jit-nested-loop-wrong-result.md.
#[test]
fn trace_audit_nested_loops() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 for j = 1, 100 do
                     s = s + 1
                 end
             end
             return s",
        );
        // 100 * 100 = 10000
        assert!(
            matches!(r, Value::Int(10000)),
            "nested[{}]: expected Int(10000), got {:?}",
            label,
            r
        );
    }
}

/// Hot loop with string concat — engages buffered concat path
/// (accumulator shape).
#[test]
fn trace_audit_string_concat_loop() {
    for (_v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local s = ''; for i = 1, 100 do s = s .. 'x' end; return s",
        );
        match r {
            Value::Str(s) => {
                assert_eq!(s.as_bytes().len(), 100, "{} concat len", label);
                assert!(
                    s.as_bytes().iter().all(|&b| b == b'x'),
                    "{} concat content",
                    label
                );
            }
            _ => panic!("concat-loop[{}]: not str: {:?}", label, r),
        }
    }
}

/// `for-each` over a table — TForLoop trace JIT path (s12 territory).
#[test]
fn trace_audit_for_each_table() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}
             for i = 1, 1000 do t[i] = i end
             local s = 0
             for _, v in ipairs(t) do s = s + v end
             return s",
        );
        // 1000 * 1001 / 2 = 500500
        assert!(
            matches!(r, Value::Int(500500)),
            "for-each-table[{}]: expected Int(500500), got {:?}",
            label,
            r
        );
    }
}

/// Hot exit + side trace shape — the inner loop's exit creates a side
/// trace candidate. FIXED alongside trace_audit_nested_loops.
#[test]
fn trace_audit_hot_exit_side_trace() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for outer = 1, 50 do
                 for inner = 1, 100 do
                     s = s + 1
                 end
             end
             return s",
        );
        // 50 * 100 = 5000
        assert!(
            matches!(r, Value::Int(5000)),
            "hot-exit[{}]: expected Int(5000), got {:?}",
            label,
            r
        );
    }
}

/// Tail-call-shaped recursion — both engines apply TCO.
#[test]
fn trace_audit_tail_recursive_count() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local function f(n, acc) if n == 0 then return acc end return f(n-1, acc + n) end; return f(100, 0)",
        );
        // sum(1..100) = 5050
        assert!(
            matches!(r, Value::Int(5050)),
            "tail-rec[{}]: expected Int(5050), got {:?}",
            label,
            r
        );
    }
}

/// `math.*` fold — trace JIT inlines a small set of math libcalls.
#[test]
fn trace_audit_math_libm_fold() {
    for (_v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local s = 0.0
             for i = 1, 1000 do s = s + math.sqrt(i) end
             return s",
        );
        match r {
            Value::Float(f) => assert!(f > 20000.0 && f < 22000.0, "{} sqrt-sum: got {}", label, f),
            _ => panic!("math-libm[{}]: not Float: {:?}", label, r),
        }
    }
}
