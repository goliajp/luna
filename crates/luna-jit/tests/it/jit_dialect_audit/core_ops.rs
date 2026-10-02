//! Audit cases — each exercises one Op pattern.

use super::*;

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
        assert!(matches!(r, Value::Bool(true)), "not-nil/{}: {:?}", label, r);
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
