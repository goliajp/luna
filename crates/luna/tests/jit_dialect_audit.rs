//! Method JIT × dialect × Value-introspection audit.
//!
//! Each test runs ONE Op pattern (or small Op combination) under every
//! Lua dialect with method JIT enabled (default), then asserts the
//! returned Value's variant matches the dialect's type semantics.
//!
//! Method JIT bugs of the form "value is Lua-level correct but the
//! runtime Value variant is wrong" (tag drift) are invisible to
//! PUC-diff testing because Lua-level `type()` / arithmetic auto-
//! promote between Int and Float. This file catches them by
//! discriminating on the actual `Value` enum variant.
//!
//! Two latent bugs were caught and fixed this session by tests of
//! this shape:
//! 1. `Op::GetTable` defaulting result kind to Int regardless of
//!    dialect (5.1/5.2 returned `Int(0x4034...)` = f64 raw bits as
//!    Int instead of `Float(20.0)`). Fixed at src/jit/mod.rs:3016
//!    (dialect-aware default).
//! 2. 5.1's `luaK_nil` optimization skips `LoadNil` for an
//!    uninitialized local at function start; method JIT then read
//!    cranelift Variable's default `0` and silently arith'd it
//!    (`nil + 1 → 1` instead of raising). Fixed at
//!    src/jit/mod.rs::try_compile_int_chunk scan-init (pre-mark
//!    `is_nil_writer[num_params..max_stack] = true`).
//!
//! Naming: `audit_<op_or_pattern>` per test.

use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::Vm;

// ---------------------------------------------------------------------------
// Helpers — by default Vm has method JIT enabled, exercising the
// JIT bug surface. To re-test interp-only, set JIT off via the
// `interp_*` variants.

fn vm_default(version: LuaVersion) -> Vm {
    luna::new_with_jit(version)
}

fn eval_one(vm: &mut Vm, src: &str) -> Value {
    let cl = vm.load(src.as_bytes(), b"=audit").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    r.into_iter().next().unwrap_or(Value::Nil)
}

fn is_pre_53(v: LuaVersion) -> bool {
    matches!(v, LuaVersion::Lua51 | LuaVersion::Lua52)
}

/// Assert numeric `n`. Under 5.3+ the variant must be `Int(n)` (strict,
/// since 5.3+ has the integer subtype). Under 5.1/5.2 accept either
/// `Int(n)` (luna's internal optimization for stdlib paths like `#tbl`,
/// `math.floor`) OR `Float(n as f64)` (PUC-strict) — both are
/// Lua-level "number" with value `n`, so `type()` and arithmetic
/// agree. Refusing the Int form here would false-positive on benign
/// tag drift; refusing the Float form would miss real bugs like the
/// `Int(raw_f64_bits_of_n)` shape the JIT GetTable bug produced.
///
/// To catch the raw-bits bug class explicitly, we also reject any Int
/// whose value differs from `n` and whose f64 bit-interpretation also
/// differs from `n` (i.e. neither an honest Int nor a tag-drift Float
/// re-interpreted as Int).
fn assert_strict_num(version: LuaVersion, actual: Value, n: i64, label: &str) {
    let ok = match actual {
        Value::Int(i) if i == n => true,
        Value::Float(f) if is_pre_53(version) && f == n as f64 => true,
        _ => false,
    };
    assert!(
        ok,
        "audit[{}]: expected number == {}, got {:?}",
        label, n, actual
    );
}

fn assert_strict_float(actual: Value, expected: f64, eps: f64, label: &str) {
    match actual {
        Value::Float(f) => assert!(
            (f - expected).abs() < eps,
            "audit[{}]: expected ≈{}, got Float({})",
            label,
            expected,
            f
        ),
        _ => panic!("audit[{}]: expected Float, got {:?}", label, actual),
    }
}

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "5.1"),
    (LuaVersion::Lua52, "5.2"),
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

// ---------------------------------------------------------------------------
// Audit cases — each exercises one Op pattern.

/// LoadI / LoadF + Add. Smoke already covers a basic version; this
/// adds explicit 5.1/5.2 Float check.
#[test]
fn audit_arith_add() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 2 + 3");
        assert_strict_num(*v, r, 5, &format!("add/{}", label));
    }
}

#[test]
fn audit_arith_mul() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 6 * 7");
        assert_strict_num(*v, r, 42, &format!("mul/{}", label));
    }
}

#[test]
fn audit_arith_sub_neg() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 10 - 100");
        assert_strict_num(*v, r, -90, &format!("sub/{}", label));
    }
}

#[test]
fn audit_arith_div() {
    // `/` is float division in all dialects (PUC 5.3+ `//` is the
    // integer one). 10/4 = 2.5 everywhere.
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 10 / 4");
        assert_strict_float(r, 2.5, 1e-12, &format!("div/{}", label));
    }
}

/// Op::Self (method call sugar). `t:m()` reads R[t] for the function
/// and passes R[t] as first arg.
#[test]
fn audit_method_call() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {x = 10}; function t:get() return self.x end; return t:get()",
        );
        assert_strict_num(*v, r, 10, &format!("method/{}", label));
    }
}

/// Op::SetTable storing an integer-literal value.
#[test]
fn audit_settable_int_value() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local t = {}; t[1] = 42; return t[1]");
        assert_strict_num(*v, r, 42, &format!("settable-int/{}", label));
    }
}

/// Op::SetTable storing a float literal.
#[test]
fn audit_settable_float_value() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        // 3.14 is unambiguously Float across all dialects (decimal point).
        let r = eval_one(&mut vm, "local t = {}; t[1] = 3.14; return t[1]");
        assert_strict_float(r, 3.14, 1e-12, &format!("settable-float/{}", label));
    }
}

/// GetTable with computed (non-immediate) key.
#[test]
fn audit_gettable_computed_key() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {10, 20, 30, 40, 50}; local i = 3; return t[i]",
        );
        assert_strict_num(*v, r, 30, &format!("gettable-computed/{}", label));
    }
}

/// Op::Move — move from one register to another. Must not change
/// the value's tag.
#[test]
fn audit_move_chain() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local a = 17; local b = a; local c = b; return c");
        assert_strict_num(*v, r, 17, &format!("move/{}", label));
    }
}

/// String concat with numeric coercion. Result is always Str.
#[test]
fn audit_concat_with_numbers() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 'x=' .. 7 .. ' y=' .. 3.14");
        match r {
            Value::Str(s) => {
                let body = String::from_utf8_lossy(s.as_bytes()).to_string();
                assert!(
                    body.starts_with("x=7 y=3.14"),
                    "concat/{}: got {:?}",
                    label,
                    body
                );
            }
            _ => panic!("concat/{}: expected Str, got {:?}", label, r),
        }
    }
}

/// Eq / Lt / Le — must return Bool variant.
#[test]
fn audit_eq_returns_bool() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 2 == 2");
        assert!(
            matches!(r, Value::Bool(true)),
            "eq/{}: expected Bool(true), got {:?}",
            label,
            r
        );
    }
}

#[test]
fn audit_lt_returns_bool() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 2 < 3");
        assert!(
            matches!(r, Value::Bool(true)),
            "lt/{}: expected Bool(true), got {:?}",
            label,
            r
        );
    }
}

/// Op::Not — boolean negation. `not nil`, `not false`, `not 0`
/// (PUC 0 is truthy).
#[test]
fn audit_not_truthy() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return not nil");
        assert!(
            matches!(r, Value::Bool(true)),
            "not-nil/{}: {:?}",
            label,
            r
        );
        let r = eval_one(&mut vm, "return not 0");
        assert!(
            matches!(r, Value::Bool(false)),
            "not-0/{}: {:?} (0 is truthy in Lua)",
            label,
            r
        );
    }
}

/// Upvalue read — closure captures local then reads it. Must preserve
/// the captured value's variant.
#[test]
fn audit_upval_read_int() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local x = 42; local function get() return x end; return get()",
        );
        assert_strict_num(*v, r, 42, &format!("upval-read/{}", label));
    }
}

#[test]
fn audit_upval_write_then_read() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local x = 1; local function set(n) x = n end; local function get() return x end; set(99); return get()",
        );
        assert_strict_num(*v, r, 99, &format!("upval-rw/{}", label));
    }
}

/// Numeric for loop with int counter — sum of 1..10 = 55.
#[test]
fn audit_for_int_counter() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 1, 10 do s = s + i end; return s",
        );
        assert_strict_num(*v, r, 55, &format!("for-int/{}", label));
    }
}

/// Numeric for loop with explicit float counter.
#[test]
fn audit_for_float_counter() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        // i=1.5, 2.5, 3.5 → sum = 7.5
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 1.5, 3.5, 1 do s = s + i end; return s",
        );
        assert_strict_float(r, 7.5, 1e-12, &format!("for-float/{}", label));
    }
}

/// Op::Len — # operator on table and string.
#[test]
fn audit_len_table() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return #{10, 20, 30, 40}");
        assert_strict_num(*v, r, 4, &format!("len-tbl/{}", label));
    }
}

#[test]
fn audit_len_string() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return #'hello world'");
        assert_strict_num(*v, r, 11, &format!("len-str/{}", label));
    }
}

/// Reading an uninitialized local should propagate Nil (this is the
/// specific bug fixed this session — JIT silently consumed 0 on 5.1).
#[test]
fn audit_uninit_local_is_nil() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "local x; return x");
        assert!(
            matches!(r, Value::Nil),
            "uninit-local/{}: expected Nil, got {:?}",
            label,
            r
        );
    }
}

/// Same as above but via a function (the bug's actual trigger shape).
#[test]
fn audit_uninit_local_via_function() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(
            &mut vm,
            "local function f() local x; return x end; return f()",
        );
        assert!(
            matches!(r, Value::Nil),
            "uninit-local-fn/{}: expected Nil, got {:?}",
            label,
            r
        );
    }
}

/// `nil + 1` raises across all dialects + execution paths. Sister
/// test to the e2e harness's `err_arith_on_nil`; this one runs at
/// the runtime-API level rather than the source-level diff.
#[test]
fn audit_arith_on_nil_raises() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let cl = vm
            .load(
                b"local function bad() local x; return x + 1 end; return bad()",
                b"=audit",
            )
            .unwrap();
        let r = vm.call_value(Value::Closure(cl), &[]);
        assert!(
            r.is_err(),
            "arith-on-nil/{}: expected Err, got Ok({:?})",
            label,
            r
        );
    }
}

/// pcall + nil arith — must return false + error message.
#[test]
fn audit_pcall_catches_nil_arith() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(
            &mut vm,
            "local function bad() local x; return x + 1 end; local ok = pcall(bad); return ok",
        );
        assert!(
            matches!(r, Value::Bool(false)),
            "pcall-nil-arith/{}: expected Bool(false), got {:?}",
            label,
            r
        );
    }
}

/// Multi-arg function — args passed via Op::Call. Verify each arg
/// preserves its variant through the call.
#[test]
fn audit_call_multi_args() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local function f(a, b, c) return a + b + c end; return f(1, 2, 3)",
        );
        assert_strict_num(*v, r, 6, &format!("call-multi/{}", label));
    }
}

/// Nested function call return — caller's return value tag must
/// match callee's.
#[test]
fn audit_nested_returns() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local function inner() return 17 end; local function outer() return inner() end; return outer()",
        );
        assert_strict_num(*v, r, 17, &format!("nested-ret/{}", label));
    }
}

/// Tail call — `return f(x)` should preserve the inner value's tag.
#[test]
fn audit_tail_call_preserves() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local function inner(n) return n * 2 end; local function outer(n) return inner(n + 1) end; return outer(20)",
        );
        assert_strict_num(*v, r, 42, &format!("tail-call/{}", label));
    }
}

/// math.floor returns Int on 5.3+, Float on 5.1/5.2 (no int subtype).
#[test]
fn audit_math_floor_dialect() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return math.floor(3.7)");
        assert_strict_num(*v, r, 3, &format!("floor/{}", label));
    }
}

/// Bitwise on 5.3+ only — gated. Result is Int across all dialects
/// that support it.
#[test]
fn audit_bitwise_53plus() {
    for (v, label) in &DIALECTS[2..] {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 0xff & 0x0f");
        match r {
            Value::Int(15) => {}
            _ => panic!("bitwise/{}: expected Int(15), got {:?}", label, r),
        }
    }
}

/// Integer division 5.3+ only. 10 // 3 = 3 as Int.
#[test]
fn audit_idiv_53plus() {
    for (v, label) in &DIALECTS[2..] {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 10 // 3");
        match r {
            Value::Int(3) => {}
            _ => panic!("idiv/{}: expected Int(3), got {:?}", label, r),
        }
    }
}

// ---------------------------------------------------------------------------
// Third round — wider Op coverage + JIT-engagement shapes.

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
        let r = eval_one(
            &mut vm,
            "local f = function() return 99 end; return f()",
        );
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
        assert!(matches!(r, Value::Bool(false)), "{} int==str: {:?}", label, r);

        // But int == float with same numeric value IS true.
        let r = eval_one(&mut vm, "return 2 == 2.0");
        assert!(matches!(r, Value::Bool(true)), "{} int==float: {:?}", label, r);

        // 1 == 1 is true.
        let r = eval_one(&mut vm, "return 1 == 1");
        assert!(matches!(r, Value::Bool(true)), "{} int==int: {:?}", label, r);
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
        assert!(matches!(r, Value::Bool(true)), "{} nil==nil: {:?}", label, r);
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

// ---------------------------------------------------------------------------
// Fourth round — Set-path audit. The prior `pre53 → float_only` fix
// only touched `Op::GetTable`. Other Set/Get paths might have similar
// dialect-default-kind issues.

/// SetTable then GetTable — round-trip int value through table.
#[test]
fn audit_settable_gettable_int() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}; t[1] = 42; local i = 1; return t[i]",
        );
        assert_strict_num(*v, r, 42, &format!("set-get-int/{}", label));
    }
}

/// SetTable then GetTable — float value through table.
#[test]
fn audit_settable_gettable_float() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(
            &mut vm,
            "local t = {}; t[1] = 3.14; local i = 1; return t[i]",
        );
        assert_strict_float(r, 3.14, 1e-12, &format!("set-get-float/{}", label));
    }
}

/// Mixed-kind SetTable then GetTable — table with both int and float
/// values at different keys.
#[test]
fn audit_settable_mixed_kinds() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}; t[1] = 10; t[2] = 20.5; local i = 1; return t[i]",
        );
        assert_strict_num(*v, r, 10, &format!("set-mixed-1/{}", label));

        let mut vm2 = vm_default(*v);
        let r = eval_one(
            &mut vm2,
            "local t = {}; t[1] = 10; t[2] = 20.5; local i = 2; return t[i]",
        );
        assert_strict_float(r, 20.5, 1e-12, &format!("set-mixed-2/{}", label));
    }
}

/// SetField via field syntax — `t.x = v`. Different BC op (Op::SetField).
#[test]
fn audit_setfield_round_trip() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local t = {}; t.x = 42; return t.x");
        assert_strict_num(*v, r, 42, &format!("setfield-int/{}", label));

        let mut vm2 = vm_default(*v);
        let r = eval_one(&mut vm2, "local t = {}; t.y = 3.14; return t.y");
        assert_strict_float(r, 3.14, 1e-12, &format!("setfield-float/{}", label));
    }
}

/// Op::SetI — immediate-int-key set (5.3+ specific BC: `t[const_i] = v`).
#[test]
fn audit_seti_immediate_key() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        // PUC compiler likely emits SetI for an immediate integer key.
        // Verify the value survives the round trip.
        let r = eval_one(
            &mut vm,
            "local t = {}; t[5] = 100; return t[5]",
        );
        assert_strict_num(*v, r, 100, &format!("seti/{}", label));
    }
}

/// Loop-driven SetTable then read — frequent JIT shape for accumulators.
#[test]
fn audit_loop_settable_read() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {}
             for i = 1, 5 do t[i] = i * 10 end
             return t[3]",
        );
        assert_strict_num(*v, r, 30, &format!("loop-set-read/{}", label));
    }
}

/// String key Set/Get cycle. Strings interned + hash-table path.
#[test]
fn audit_setfield_string_value() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(
            &mut vm,
            "local t = {}; t.name = 'bob'; return t.name",
        );
        match r {
            Value::Str(s) => assert_eq!(s.as_bytes(), b"bob", "{}", label),
            _ => panic!("setfield-str/{}: not a string: {:?}", label, r),
        }
    }
}

/// Concat preserves the result as Str across all dialects.
#[test]
fn audit_concat_in_loop() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(
            &mut vm,
            "local parts = {}
             for i = 1, 5 do parts[i] = 'x' end
             return table.concat(parts)",
        );
        match r {
            Value::Str(s) => assert_eq!(s.as_bytes(), b"xxxxx", "{}", label),
            _ => panic!("concat-loop/{}: not str: {:?}", label, r),
        }
    }
}

/// Nested tables.
#[test]
fn audit_nested_table_access() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {inner = {value = 99}}; return t.inner.value",
        );
        assert_strict_num(*v, r, 99, &format!("nested-tbl/{}", label));
    }
}

/// Setting a table value through metatable __newindex (5.x).
#[test]
fn audit_setindex_metatable() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local backing = {}
             local t = setmetatable({}, {__newindex = backing})
             t.x = 77
             return backing.x",
        );
        assert_strict_num(*v, r, 77, &format!("mt-newindex/{}", label));
    }
}

/// Reading from a metatable __index chain.
#[test]
fn audit_getindex_metatable_chain() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local base = {magic = 88}
             local t = setmetatable({}, {__index = base})
             return t.magic",
        );
        assert_strict_num(*v, r, 88, &format!("mt-index-chain/{}", label));
    }
}

/// Loop accumulating into a table sum (probe accumulator + JIT trace).
#[test]
fn audit_loop_table_accumulate() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local t = {0}
             for i = 1, 100 do t[1] = t[1] + i end
             return t[1]",
        );
        assert_strict_num(*v, r, 5050, &format!("loop-accum/{}", label));
    }
}

/// Numeric for with explicit step.
#[test]
fn audit_for_with_step() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 2, 20, 2 do s = s + i end; return s",
        );
        // sum(2, 4, ..., 20) = 2 * sum(1..10) = 110
        assert_strict_num(*v, r, 110, &format!("for-step/{}", label));
    }
}

/// Reverse numeric for (step = -1).
#[test]
fn audit_for_reverse() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(
            &mut vm,
            "local s = 0; for i = 10, 1, -1 do s = s + i end; return s",
        );
        assert_strict_num(*v, r, 55, &format!("for-reverse/{}", label));
    }
}

/// Math operations preserve Int kind on 5.3+ when both operands are Int.
#[test]
fn audit_int_int_arith_53plus() {
    for (v, label) in &DIALECTS[2..] {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "return 7 + 3");
        // 5.3+: Int+Int=Int strictly.
        assert!(
            matches!(r, Value::Int(10)),
            "int-int-arith-strict-int/{}: expected Int(10), got {:?}",
            label,
            r
        );
    }
}

/// Float + Int on 5.3+ → Float (promotion).
#[test]
fn audit_int_float_promotion_53plus() {
    for (_v, label) in &DIALECTS[2..] {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 7 + 0.5");
        assert!(
            matches!(r, Value::Float(f) if f == 7.5),
            "int-float-promote/{}: expected Float(7.5), got {:?}",
            label,
            r
        );
    }
}

/// Modulo preserves Int on 5.3+.
#[test]
fn audit_mod_int_53plus() {
    for (_v, label) in &DIALECTS[2..] {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 17 % 5");
        assert!(
            matches!(r, Value::Int(2)),
            "mod-int/{}: expected Int(2), got {:?}",
            label,
            r
        );
    }
}

/// Power operation always returns Float (PUC spec).
#[test]
fn audit_pow_always_float() {
    for (_v, label) in DIALECTS {
        let mut vm = vm_default(*_v);
        let r = eval_one(&mut vm, "return 2 ^ 10");
        assert!(
            matches!(r, Value::Float(f) if f == 1024.0),
            "pow/{}: expected Float(1024.0), got {:?}",
            label,
            r
        );
    }
}
