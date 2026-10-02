//! Fourth round — Set-path audit. The prior `pre53 → float_only` fix
//! only touched `Op::GetTable`. Other Set/Get paths might have similar
//! dialect-default-kind issues.

use super::*;

/// SetTable then GetTable — round-trip int value through table.
#[test]
fn audit_settable_gettable_int() {
    for (v, label) in DIALECTS {
        let mut vm = vm_default(*v);
        let r = eval_one(&mut vm, "local t = {}; t[1] = 42; local i = 1; return t[i]");
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
        let r = eval_one(&mut vm, "local t = {}; t[5] = 100; return t[5]");
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
        let r = eval_one(&mut vm, "local t = {}; t.name = 'bob'; return t.name");
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
