//! Round 3 — trace correctness vs interp parity. Run the same program
//! twice on the same Vm — JIT cache warmed on first run, hot on second.
//! Both should produce identical results.

use super::*;

/// Run program N times on same Vm — confirms JIT and interp produce
/// identical results across compile boundaries.
#[test]
fn trace_audit_repeated_runs_consistent() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let src = "local s = 0; for i = 1, 500 do s = s + i*i end; return s";
        let cl = vm.load(src.as_bytes(), b"=p").unwrap();
        let cl_val = Value::Closure(cl);
        let r1 = vm.call_value(cl_val, &[]).unwrap()[0];
        let r2 = vm.call_value(cl_val, &[]).unwrap()[0];
        let r3 = vm.call_value(cl_val, &[]).unwrap()[0];
        // sum(i^2, i=1..500) = 500*501*1001/6 = 41791750
        assert!(
            matches!(r1, Value::Int(41_791_750))
                && matches!(r2, Value::Int(41_791_750))
                && matches!(r3, Value::Int(41_791_750)),
            "repeated[{}]: expected all Int(41791750), got {:?}, {:?}, {:?}",
            label,
            r1,
            r2,
            r3
        );
    }
}

/// JIT disabled vs JIT enabled — same result.
#[test]
fn trace_audit_jit_off_vs_on_same_result() {
    let src =
        b"local function f(n) if n < 2 then return n end return f(n-1) + f(n-2) end; return f(18)";
    for (v, label) in POST53_DIALECTS {
        let mut vm_off = vm_default(*v);
        vm_off.set_jit_enabled(false);
        vm_off.set_trace_jit_enabled(false);
        let cl = vm_off.load(src, b"=p").unwrap();
        let r_off = vm_off.call_value(Value::Closure(cl), &[]).unwrap()[0];

        let mut vm_on = vm_default(*v);
        let cl = vm_on.load(src, b"=p").unwrap();
        let r_on = vm_on.call_value(Value::Closure(cl), &[]).unwrap()[0];

        assert!(
            matches!(r_off, Value::Int(2584)),
            "{} JIT-off: expected Int(2584), got {:?}",
            label,
            r_off
        );
        assert!(
            matches!(r_on, Value::Int(2584)),
            "{} JIT-on:  expected Int(2584), got {:?}",
            label,
            r_on
        );
    }
}

/// Method JIT only (trace JIT off) vs both on.
#[test]
fn trace_audit_method_only_vs_both() {
    let src = b"local s = 0; for i = 1, 200 do s = s + i end; return s";
    for (v, label) in POST53_DIALECTS {
        let mut vm_m = vm_default(*v);
        vm_m.set_trace_jit_enabled(false);
        let cl = vm_m.load(src, b"=p").unwrap();
        let r_m = vm_m.call_value(Value::Closure(cl), &[]).unwrap()[0];

        let mut vm_b = vm_default(*v);
        let cl = vm_b.load(src, b"=p").unwrap();
        let r_b = vm_b.call_value(Value::Closure(cl), &[]).unwrap()[0];

        // sum(1..200) = 20100
        assert!(
            matches!(r_m, Value::Int(20100)) && matches!(r_b, Value::Int(20100)),
            "method-vs-both[{}]: M={:?} B={:?}",
            label,
            r_m,
            r_b
        );
    }
}

/// Trace correctness on a workload with side-exit chains. The PUC test
/// `binary_trees`-style recursion stresses trace creation + side-exit
/// dispatch.
#[test]
fn trace_audit_binary_trees_mini() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local function make(d) if d == 0 then return nil end return {make(d-1), make(d-1)} end
             local function chk(n) if n == nil then return 0 end return 1 + chk(n[1]) + chk(n[2]) end
             return chk(make(8))",
        );
        // make(d) builds 2^d - 1 nodes; make(8) = 255.
        assert!(
            matches!(r, Value::Int(255)),
            "btrees_8[{}]: expected Int(255), got {:?}",
            label,
            r
        );
    }
}

/// Trace abort + reset — programs that engage but then abort should
/// fall back to interp cleanly and still produce correct results.
/// Mixed-type accumulator forces trace abort.
#[test]
fn trace_audit_mixed_type_accumulator() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0
             for i = 1, 100 do
                 if i % 5 == 0 then s = s + i * 1.5  -- Float branch
                 else s = s + i end                    -- Int branch
             end
             return s",
        );
        // sum(1..100) = 5050
        // 5-multiples (5,10,...,100): 20 values, sum = 1050, scaled by 1.5 → 1575
        // non-5-multiples sum: 5050 - 1050 = 4000
        // total: 4000 + 1575 = 5575
        let ok = match r {
            Value::Int(5575) => true,
            Value::Float(f) => (f - 5575.0).abs() < 1e-9,
            _ => false,
        };
        assert!(ok, "mixed-acc[{}]: expected 5575, got {:?}", label, r);
    }
}

/// Specifically check: trace recorder doesn't lose state across a
/// `pcall` boundary. The pcall'd body is hot and engages trace; verify
/// result.
#[test]
fn trace_audit_pcall_wrapping_hot_body() {
    for (v, label) in POST53_DIALECTS {
        let mut vm = vm_trace_only(*v);
        let r = eval_one(
            &mut vm,
            "local ok, v = pcall(function()
                 local s = 0
                 for i = 1, 1000 do s = s + i end
                 return s
             end)
             return ok and v or -1",
        );
        // pcall succeeds + sum(1..1000) = 500500
        assert!(
            matches!(r, Value::Int(500500)),
            "pcall-hot[{}]: expected Int(500500), got {:?}",
            label,
            r
        );
    }
}
