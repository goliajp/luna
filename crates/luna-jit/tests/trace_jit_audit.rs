//! Trace JIT correctness audit — exercises hot-loop / side-trace /
//! invalidation / cross-trace shapes that the method-JIT audit doesn't
//! engage. Each test:
//! 1. Runs a Lua program that SHOULD engage trace JIT recording.
//! 2. Verifies the result is correct (Lua-level + Value variant).
//! 3. Optionally inspects trace counters to confirm engagement (so a
//!    silently-bailed trace doesn't pass as "Lua-level correct via
//!    interp fallback").
//!
//! These tests are dialect-light — most trace JIT shapes are dialect-
//! agnostic, but a few are 5.4+ only (ForLoop pre-5.3 uses a different
//! BC layout that trace JIT bails on).

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn vm_default(version: LuaVersion) -> Vm {
    luna_jit::new_with_jit(version)
}

/// luna's default Vm enables BOTH method JIT and trace JIT. Method
/// JIT consumes a proto on first call if it can; trace JIT only sees
/// protos method JIT BAILED on. To audit trace JIT specifically, we
/// must disable method JIT so trace JIT runs.
fn vm_trace_only(version: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm
}

fn eval_one(vm: &mut Vm, src: &str) -> Value {
    let cl = vm.load(src.as_bytes(), b"=trace-audit").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    r.into_iter().next().unwrap_or(Value::Nil)
}

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "5.1"),
    (LuaVersion::Lua52, "5.2"),
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

/// 5.4 / 5.5 only — pre53 ForLoop layout differs and trace JIT bails
/// (`src/jit/trace.rs:4410`).
const POST53_DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

// ---------------------------------------------------------------------------
// Round 1 — basic hot-loop traces compile + dispatch correctly.

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
/// (S14-B accumulator shape).
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

// ---------------------------------------------------------------------------
// Round 2 — pre-5.3 dialect handling. trace JIT bails on pre-5.3
// ForLoop; programs must still produce correct Lua-level results via
// the interp fallback.

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

// ---------------------------------------------------------------------------
// Round 3 — trace correctness vs interp parity. Run the same program
// twice on the same Vm — JIT cache warmed on first run, hot on second.
// Both should produce identical results.

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

// ---------------------------------------------------------------------------
// Round 4 — ForLoop body_pc fix corner cases. These exercise less-
// common but valid ForLoop shapes to make sure the fix doesn't break
// edge cases.

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
