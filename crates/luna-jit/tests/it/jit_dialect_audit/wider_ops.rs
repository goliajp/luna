//! Third round — wider Op coverage + JIT-engagement shapes.

use super::*;

/// Op::GetField — `t.x` (string-key indexing, immediate string constant).
/// Different from GetTable: the key is encoded in the constant pool, not
/// a register. Easy place for tag drift to escape if the field-fetch
/// path has its own analysis branch.
#[test]
fn audit_getfield_int_value() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local t = {x = 42}; return t.x");
        assert_strict_num(*v, r, 42, &format!("getfield-int/{}", label));
    }
}

#[test]
fn audit_getfield_float_value() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "local t = {y = 3.14}; return t.y");
        assert_strict_float(r, 3.14, 1e-12, &format!("getfield-float/{}", label));
    }
}

#[test]
fn audit_getfield_string_value() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "local t = {name = 'alice'}; return t.name");
        match r {
            Value::Str(s) => assert_eq!(s.as_bytes(), b"alice", "{}", label),
            _ => panic!("getfield-str/{}: expected Str, got {:?}", label, r),
        }
    }
}

/// Op::Closure — `function() ... end` materializes a closure. Verify
/// the closure's return value variant flows through correctly.
#[test]
fn audit_closure_return() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local f = function() return 99 end; return f()");
        assert_strict_num(*v, r, 99, &format!("closure-ret/{}", label));
    }
}

/// Multiple closures over the same upval — closing-over invariant test.
#[test]
fn audit_two_closures_share_upval() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local count = 0
             local function inc() count = count + 1 end
             local function get() return count end
             inc(); inc(); inc()
             return get()",
        );
        assert_strict_num(*v, r, 3, &format!("two-closures-upval/{}", label));
    }
}

/// Op::TForLoop — generic for. The `for k,v in pairs(t) do ... end`
/// pattern. JIT-engagement of TForLoop has had recent fixes
/// (s12 chain) — verify still correct across dialects.
#[test]
fn audit_tfor_pairs_sum() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {a=1, b=2, c=3, d=4}; local s = 0
             for k, v in pairs(t) do s = s + v end
             return s",
        );
        assert_strict_num(*v, r, 10, &format!("tfor-pairs/{}", label));
    }
}

#[test]
fn audit_tfor_ipairs_sum() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {10, 20, 30, 40, 50}; local s = 0
             for i, v in ipairs(t) do s = s + v end
             return s",
        );
        assert_strict_num(*v, r, 150, &format!("tfor-ipairs/{}", label));
    }
}

/// Hot loop (>=N iterations) — engages trace JIT recorder. Verify
/// returned variant survives the trace compile + dispatch round trip.
#[test]
fn audit_hot_loop_int_sum() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 1, 1000 do s = s + i end; return s",
        );
        // 1000 * 1001 / 2 = 500500
        assert_strict_num(*v, r, 500500, &format!("hot-loop/{}", label));
    }
}

/// Op::Not + Op::TestSet — `a and b` / `a or b` shapes. The truthy
/// path matters: 0, "", {} are truthy in Lua (unlike Python).
#[test]
fn audit_and_or_truthy() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);

        // `nil and X` → nil
        let r = eval_one(&mut vm, "return nil and 42");
        assert!(matches!(r, Value::Nil), "{} nil-and: {:?}", label, r);

        // `false or X` → X
        let r = eval_one(&mut vm, "return false or 42");
        assert!(
            matches!(r, Value::Int(42) | Value::Float(_)),
            "{} false-or: {:?}",
            label,
            r
        );

        // `0 and X` → X (0 is truthy in Lua)
        let r = eval_one(&mut vm, "return 0 and 42");
        assert!(
            matches!(r, Value::Int(42) | Value::Float(_)),
            "{} 0-and: {:?}",
            label,
            r
        );

        // `"" and X` → X (empty string is truthy)
        let r = eval_one(&mut vm, "return '' and 42");
        assert!(
            matches!(r, Value::Int(42) | Value::Float(_)),
            "{} empty-str-and: {:?}",
            label,
            r
        );
    }
}

/// Op::Concat — string concat chain. Lua's concat is right-associative;
/// JIT may emit a buffered shape. Verify final string is correct.
#[test]
fn audit_concat_chain() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 'a' .. 'b' .. 'c' .. 'd' .. 'e'");
        match r {
            Value::Str(s) => assert_eq!(s.as_bytes(), b"abcde", "{}", label),
            _ => panic!("concat-chain/{}: not a string: {:?}", label, r),
        }
    }
}

/// Returning multiple values. Each return slot must keep its tag.
#[test]
fn audit_multi_return() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let mut vm2 = vm_default(*v);
        // First slot — verify variant.
        let r = eval_one(
            &mut vm,
            "local function f() return 1, 2, 3 end; return (f())",
        );
        assert_strict_num(*v, r, 1, &format!("multi-ret-1st/{}", label));
        // Sum via Lua-level — exercise multi-return + select.
        let r = eval_one(
            &mut vm2,
            "local function f() return 1, 2, 3 end; local a, b, c = f(); return a + b + c",
        );
        assert_strict_num(*v, r, 6, &format!("multi-ret-sum/{}", label));
    }
}

/// Op::Eq with cross-type — `1 == "1"` is FALSE in Lua (no coercion
/// for ==). Common confusion point for JIT type-specialization.
#[test]
fn audit_eq_no_cross_type_coercion() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);

        let r = eval_one(&mut vm, "return 1 == '1'");
        assert!(
            matches!(r, Value::Bool(false)),
            "{} int==str: {:?}",
            label,
            r
        );

        // But int == float with same numeric value IS true.
        let r = eval_one(&mut vm, "return 2 == 2.0");
        assert!(
            matches!(r, Value::Bool(true)),
            "{} int==float: {:?}",
            label,
            r
        );

        // 1 == 1 is true.
        let r = eval_one(&mut vm, "return 1 == 1");
        assert!(
            matches!(r, Value::Bool(true)),
            "{} int==int: {:?}",
            label,
            r
        );
    }
}

/// Trace JIT engagement — recursive function (the trace-JIT canonical
/// shape). fib(15) is small enough not to OOM but big enough to be
/// recorded as a hot trace.
#[test]
fn audit_fib_recursion_result() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local function f(n) if n < 2 then return n end return f(n-1) + f(n-2) end; return f(15)",
        );
        // fib(15) = 610
        assert_strict_num(*v, r, 610, &format!("fib15/{}", label));
    }
}

/// JIT'd function being called repeatedly — caches + dispatch path.
#[test]
fn audit_repeated_call() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local function f(x) return x * 2 end
             local s = 0
             for i = 1, 100 do s = s + f(i) end
             return s",
        );
        // sum(2..200 step 2) = 2 * sum(1..100) = 2 * 5050 = 10100
        assert_strict_num(*v, r, 10100, &format!("repeated-call/{}", label));
    }
}

/// `Op::Eq` against `nil` — special case.
#[test]
fn audit_eq_nil_check() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "local x; return x == nil");
        assert!(
            matches!(r, Value::Bool(true)),
            "{} nil==nil: {:?}",
            label,
            r
        );
        let r = eval_one(&mut vm, "return 0 == nil");
        assert!(matches!(r, Value::Bool(false)), "{} 0==nil: {:?}", label, r);
    }
}

/// Op::LoadK with float constant — `return 3.14`.
#[test]
fn audit_load_float_const() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 3.14");
        assert_strict_float(r, 3.14, 1e-12, &format!("load-fconst/{}", label));
    }
}

/// Op::LoadK with negative literal — verify sign survives.
#[test]
fn audit_load_negative_int() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return -42");
        assert_strict_num(*v, r, -42, &format!("load-neg/{}", label));
    }
}
