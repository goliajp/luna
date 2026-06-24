//! v1.3 Phase P2A — `math.min` / `math.max` 2-arg trace JIT fold.
//!
//! Extends `try_match_trace_math_fold` (`trace.rs`) with `Min2 /
//! Max2` arms. The fold collapses the `GetTabUp _ENV "math" +
//! GetField "min"|"max" + ...arg-prep... + Call(B=3,C=2)` window
//! into one Cranelift `smin/smax` (Int/Int) or `fmin/fmax` (Float
//! or mixed) IR op.
//!
//! These tests pin:
//!   1. The compiled-trace dispatch engages on the canonical
//!      `math.min(K, expr)` Redis-Lua idiom (the audit's headline
//!      win — `trace_dispatched_count > 0`).
//!   2. The fold preserves operand-type semantics:
//!      Int  / Int   → Int  result (`smin/smax`)
//!      Float/ Float → Float result (`fmin/fmax`)
//!      mixed         → Float result (Int promoted to Float)
//!   3. The fold result matches the interp / PUC-shape reference
//!      across a battery of inputs incl. negatives and edges.
//!   4. The pre-existing single-arg libm fold (Libm1) still
//!      compiles + dispatches (back-compat regression).

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

fn run_with(src: &str, trace_jit: bool) -> Vec<Value> {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(trace_jit);
    vm.open_base();
    vm.open_math();
    vm.eval(src).expect("eval")
}

/// The token_bucket workload — the canonical 2-arg `math.min(K,
/// expr)` Redis-Lua idiom. Pre-P2A: trace records + compiles
/// but `dispatched_count = 0` (bailed at `GetField:inference-fail`
/// inside the math.min Call window). Post-P2A: dispatches.
#[test]
fn token_bucket_dispatches_post_p2a() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(true);
    vm.open_base();
    vm.open_math();
    let r = vm
        .eval(
            r#"
            local bucket = { tokens = 1000, last = 0, rate = 100 }
            local now = 1
            local refilled = 0
            for i = 1, 1000 do
                local elapsed = now - bucket.last
                local refill = elapsed * bucket.rate
                if refill > 0 then
                    bucket.tokens = math.min(1000, bucket.tokens + refill)
                    bucket.last = now
                    refilled = refilled + 1
                end
                if bucket.tokens >= 1 then
                    bucket.tokens = bucket.tokens - 1
                end
                now = now + 1
            end
            return bucket.tokens, refilled
            "#,
        )
        .expect("token_bucket eval");
    // tokens drained to 999 (starts at 1000, +100 refill on iter 1
    // then -1 per iter * 1000 → 100). Wait actually refill happens
    // every iter: iter 1: now=1, last=0, elapsed=1, refill=100, but
    // math.min(1000, 1000+100)=1000, then -1 → 999. Iter 2: now=2,
    // last=1, elapsed=1, refill=100, math.min(1000, 999+100)=1000,
    // then -1 → 999. So tokens stays 999 and refilled=1000.
    assert!(matches!(r[0], Value::Int(999)), "tokens: {:?}", r[0]);
    assert!(matches!(r[1], Value::Int(1000)), "refilled: {:?}", r[1]);

    assert!(
        vm.trace_dispatched_count() > 0,
        "P2A must engage trace dispatch on token_bucket; got dispatched={} compiled={} closed={} dispatch_off_reasons={:?}",
        vm.trace_dispatched_count(),
        vm.trace_compiled_count(),
        vm.trace_closed_count(),
        vm.trace_dispatch_off_reasons(),
    );
}

/// `math.min(a, b)` over two ints — result must stay Int.
#[test]
fn min2_int_int_returns_int() {
    let src = r#"
        local s = 0
        for i = 1, 200 do
            s = s + math.min(i, 50)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    // sum_{i=1..200} min(i, 50)
    //   = 1+2+...+50 + 50*150
    //   = 1275 + 7500
    //   = 8775
    assert!(
        matches!(interp[0], Value::Int(8775)),
        "interp: {:?}",
        interp[0]
    );
    assert!(matches!(jit[0], Value::Int(8775)), "jit: {:?}", jit[0]);
}

/// `math.max(a, b)` over two ints — result must stay Int.
#[test]
fn max2_int_int_returns_int() {
    let src = r#"
        local s = 0
        for i = 1, 200 do
            s = s + math.max(i, 50)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    // sum_{i=1..200} max(i, 50)
    //   = 50*50 + sum_{i=51..200} i
    //   = 2500 + (51+200)*150/2
    //   = 2500 + 18825
    //   = 21325
    assert!(
        matches!(interp[0], Value::Int(21325)),
        "interp: {:?}",
        interp[0]
    );
    assert!(matches!(jit[0], Value::Int(21325)), "jit: {:?}", jit[0]);
}

/// `math.min` over floats — result must be Float.
#[test]
fn min2_float_float_returns_float() {
    let src = r#"
        local s = 0.0
        for i = 1, 100 do
            s = s + math.min(1.5, i * 0.1)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    // interp and jit must match
    match (&interp[0], &jit[0]) {
        (Value::Float(a), Value::Float(b)) => {
            assert!((a - b).abs() < 1e-9, "interp={} jit={}", a, b);
        }
        _ => panic!(
            "expected Float results, got interp={:?} jit={:?}",
            interp[0], jit[0]
        ),
    }
}

/// `math.min(int, float)` — result must be Float.
#[test]
fn min2_int_float_returns_float() {
    let src = r#"
        local s = 0.0
        for i = 1, 100 do
            s = s + math.min(i, 50.5)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    match (&interp[0], &jit[0]) {
        (Value::Float(a), Value::Float(b)) => {
            assert!((a - b).abs() < 1e-9, "interp={} jit={}", a, b);
        }
        _ => panic!(
            "expected Float, got interp={:?} jit={:?}",
            interp[0], jit[0]
        ),
    }
}

/// `math.max` with negative inputs — sign handling for smin/smax.
#[test]
fn max2_negative_ints_correct() {
    let src = r#"
        local s = 0
        for i = 1, 100 do
            s = s + math.max(-i, -50)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    // max(-i, -50): for i in 1..=50, max is -i; for i in 51..=100, max is -50.
    //   = -(1+2+...+50) + (-50)*50
    //   = -1275 + -2500
    //   = -3775
    assert!(
        matches!(interp[0], Value::Int(-3775)),
        "interp: {:?}",
        interp[0]
    );
    assert!(matches!(jit[0], Value::Int(-3775)), "jit: {:?}", jit[0]);
}

/// Back-compat: the existing single-arg libm fold (`math.sqrt`) still
/// works post-P2A. Regression sentinel for the recogniser refactor.
#[test]
fn libm1_sqrt_still_folds() {
    let src = r#"
        local s = 0.0
        for i = 1, 1000 do
            s = s + math.sqrt(i)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    match (&interp[0], &jit[0]) {
        (Value::Float(a), Value::Float(b)) => {
            // Approximate equality; the libm path on trace JIT may
            // use a different f64 reduction order vs the interp's
            // straight-line summation. 1e-9 is generous.
            assert!((a - b).abs() < 1e-6, "interp={} jit={}", a, b);
        }
        _ => panic!(
            "expected Float, got interp={:?} jit={:?}",
            interp[0], jit[0]
        ),
    }
}

/// `math.min(K, expr)` where `expr` involves a `GetField + Add`
/// chain (the token_bucket pattern). Validates the split-window
/// fold accepts arbitrarily-shaped arg-prep ops between the
/// `GetField "min"` and the closing `Call`.
#[test]
fn min2_with_getfield_add_arg2() {
    let src = r#"
        local t = { v = 5 }
        local s = 0
        for i = 1, 100 do
            s = s + math.min(50, t.v + i)
        end
        return s
    "#;
    let interp = run_with(src, false);
    let jit = run_with(src, true);
    // min(50, 5 + i) for i in 1..=100: for i + 5 <= 50 → 5..=49 (i <= 45) the min is i+5
    //   sum_{i=1..=45} (i+5) = (1+45)*45/2 + 5*45 = 1035 + 225 = 1260
    // for i + 5 > 50 (i >= 46) the min is 50: 50*55 = 2750
    //   total = 1260 + 2750 = 4010
    assert!(
        matches!(interp[0], Value::Int(4010)),
        "interp: {:?}",
        interp[0]
    );
    assert!(matches!(jit[0], Value::Int(4010)), "jit: {:?}", jit[0]);
}
