//! Round 4 — ForLoop body_pc fix corner cases. These exercise less-
//! common but valid ForLoop shapes to make sure the fix doesn't break
//! edge cases.

use super::*;

/// ForLoop with non-1 step. body_pc formula must still route correctly.
#[test]
fn trace_audit_for_step_2() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 for j = 1, 200, 2 do s = s + 1 end
             end
             return s",
        );
        // j: 1, 3, 5, ..., 199 = 100 iterations, × 100 outer = 10000
        assert!(
            matches!(r, Value::Int(10000)),
            "for-step-2[{}]: expected Int(10000), got {:?}",
            label,
            r
        );
    }
}

/// Reverse step ForLoop nested.
#[test]
fn trace_audit_for_reverse_nested() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 50 do
                 for j = 100, 1, -1 do s = s + 1 end
             end
             return s",
        );
        // j: 100, 99, ..., 1 = 100 iters × 50 outer = 5000
        assert!(
            matches!(r, Value::Int(5000)),
            "for-reverse-nested[{}]: expected Int(5000), got {:?}",
            label,
            r
        );
    }
}

/// 3-level nested ForLoop. Bug class might cascade across multiple
/// dispatch boundaries.
#[test]
fn trace_audit_3_level_nested() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for a = 1, 10 do
                 for b = 1, 10 do
                     for c = 1, 10 do s = s + 1 end
                 end
             end
             return s",
        );
        // 10 × 10 × 10 = 1000
        assert!(
            matches!(r, Value::Int(1000)),
            "3-nested[{}]: expected Int(1000), got {:?}",
            label,
            r
        );
    }
}

/// Float-counter inner loop with int-counter outer loop. FIXED at
/// `src/jit/trace.rs::try_compile_trace_with_options` validation:
/// trace JIT now bails on Float ForLoop (entry_tags[A] == FLOAT) so
/// interp handles it correctly. Previously trace JIT compiled Float
/// ForLoop with Int-count semantics, treating R[A+1]=limit (Float
/// bits) as a large positive Int count → `count > 0` always true →
/// infinite back-edge loop inside the trace.
#[test]
fn trace_audit_mixed_int_float_nested() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 50 do
                 for j = 1.5, 100.5 do s = s + 1 end
             end
             return s",
        );
        // j: 1.5, 2.5, ..., 100.5 = 100 iters × 50 outer = 5000
        let ok = match r {
            Value::Int(5000) => true,
            Value::Float(f) => (f - 5000.0).abs() < 1e-9,
            _ => false,
        };
        assert!(
            ok,
            "mixed-int-float-nested[{}]: expected 5000, got {:?}",
            label, r
        );
    }
}

/// TForLoop nested inside numeric ForLoop. Tests TForLoop's continue
/// path doesn't share the ForLoop body_pc bug (the fix is specifically
/// for ForLoop; TForLoop has separate continue logic).
#[test]
fn trace_audit_tforloop_in_for() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}
             for i = 1, 100 do t[i] = i end
             local s = 0
             for outer = 1, 50 do
                 for _, v in ipairs(t) do s = s + v end
             end
             return s",
        );
        // sum(1..100) = 5050, × 50 outer = 252500
        assert!(
            matches!(r, Value::Int(252500)),
            "tfor-in-for[{}]: expected Int(252500), got {:?}",
            label,
            r
        );
    }
}

/// Hot loop in nested function — function call boundary inside a
/// ForLoop body. Trace JIT may compile each function separately.
#[test]
fn trace_audit_nested_fn_call_in_loop() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local function inner_sum(n)
                 local s = 0
                 for i = 1, n do s = s + i end
                 return s
             end
             local total = 0
             for outer = 1, 50 do total = total + inner_sum(20) end
             return total",
        );
        // sum(1..20) = 210, × 50 outer = 10500
        assert!(
            matches!(r, Value::Int(10500)),
            "nested-fn-in-loop[{}]: expected Int(10500), got {:?}",
            label,
            r
        );
    }
}

/// Pre-5.3 nested loops. trace JIT bails on pre-5.3 ForLoop, so all
/// processing falls back to interp. Verify result is correct under
/// 5.1/5.2 too.
#[test]
fn trace_audit_pre53_nested_loops_interp() {
    for (_v, label) in &DIALECTS[..2] {
        let mut vm = vm_trace_only(*_v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 for j = 1, 100 do s = s + 1 end
             end
             return s",
        );
        let ok = match r {
            Value::Int(10000) => true,
            Value::Float(f) => (f - 10000.0).abs() < 1e-9,
            _ => false,
        };
        assert!(ok, "pre53-nested[{}]: expected ~10000, got {:?}", label, r);
    }
}

/// Inner ForLoop body itself contains a ForLoop break (which uses
/// goto). Mixed control flow inside the hot loop.
#[test]
fn trace_audit_break_inside_inner_loop() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 for j = 1, 100 do
                     s = s + 1
                     if j > 50 then break end
                 end
             end
             return s",
        );
        // Each outer iter runs j=1..51, then break. So 51 inner per outer.
        // Total = 100 * 51 = 5100.
        assert!(
            matches!(r, Value::Int(5100)),
            "break-in-loop[{}]: expected Int(5100), got {:?}",
            label,
            r
        );
    }
}

/// Default Vm config (both JITs enabled) — sanity check the fix
/// didn't break the path users actually exercise.
#[test]
fn trace_audit_default_vm_nested_still_correct() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v); // both JITs
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 for j = 1, 100 do s = s + 1 end
             end
             return s",
        );
        let ok = match r {
            Value::Int(10000) => true,
            Value::Float(f) => (f - 10000.0).abs() < 1e-9,
            _ => false,
        };
        assert!(
            ok,
            "default-vm-nested[{}]: expected 10000, got {:?}",
            label, r
        );
    }
}
