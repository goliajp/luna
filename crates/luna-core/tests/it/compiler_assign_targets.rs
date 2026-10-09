//! Assignment targets use registers as PUC's `restassign` does: a table or
//! key held in a variable is read from the variable when the store runs,
//! and is copied first only when a later target of the same statement
//! assigns that variable (`check_conflict`).
//!
//! Each test compiles a snippet, counts the `Move`s out of a variable's
//! register, and runs the snippet to check the stored value.

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::{Heap, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn compile_main(src: &str) -> Vec<Inst> {
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    let mut heap = Heap::new();
    let proto = compile_chunk(&ast, LuaVersion::Lua55, b"=index_lhs", &mut heap).expect("compile");
    proto.code.to_vec()
}

/// The `Move`s out of register `src_reg`.
fn count_moves_from_reg(code: &[Inst], src_reg: u32) -> usize {
    code.iter()
        .filter(|i| matches!(i.op(), Op::Move) && i.b() == src_reg)
        .count()
}

/// All the `Move`s.
fn count_all_moves(code: &[Inst]) -> usize {
    code.iter().filter(|i| matches!(i.op(), Op::Move)).count()
}

fn eval_int(src: &str) -> i64 {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut vals = match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    };
    assert_eq!(vals.len(), 1, "expected 1 returned value from {src:?}");
    match vals.pop().unwrap() {
        Value::Int(i) => i,
        other => panic!("expected Int from {src:?}, got {other:?}"),
    }
}

fn eval_int_pair(src: &str) -> (i64, i64) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut vals = match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    };
    assert_eq!(vals.len(), 2, "expected 2 returned values from {src:?}");
    let b = vals.pop().unwrap();
    let a = vals.pop().unwrap();
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => (a, b),
        other => panic!("expected (Int, Int) from {src:?}, got {other:?}"),
    }
}

// =====================================================================
// A table held in a variable is not copied
// =====================================================================

#[test]
fn a_self_decrement_takes_no_moves() {
    // `bucket.tokens = bucket.tokens - 1`: the Sub lands in a temporary
    // the SetField reads
    let src = r#"
        local bucket = { tokens = 100 }
        bucket.tokens = bucket.tokens - 1
        return bucket.tokens
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(count_all_moves(&code), 0, "no other Moves expected either");
    assert_eq!(eval_int(src), 99);
}

#[test]
fn a_math_min_call_leaves_the_table_in_place() {
    // `bucket.tokens = math.min(...)`
    let src = r#"
        local bucket = { tokens = 0 }
        bucket.tokens = math.min(1000, bucket.tokens + 10)
        return bucket.tokens
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(eval_int(src), 10);
}

#[test]
fn a_local_value_is_stored_from_its_register() {
    // `bucket.last = now`: SetField reads both `bucket` and `now` where
    // they are
    let src = r#"
        local bucket = { last = 0 }
        local now = 7
        bucket.last = now
        return bucket.last
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(count_moves_from_reg(&code, 1), 0, "now@r1 copied");
    assert_eq!(eval_int(src), 7);
}

#[test]
fn a_user_call_on_the_right_leaves_the_table_in_place() {
    // `bucket.x = f()`: the store reads `bucket` when it runs, after the
    // call
    let src = r#"
        local bucket = { x = 1 }
        local function f() return 42 end
        bucket.x = f()
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(eval_int(src), 42);
}

#[test]
fn a_method_call_on_the_right_leaves_the_table_in_place() {
    // `bucket.x = ("a"):byte(1)`
    let src = r#"
        local bucket = { x = 0 }
        bucket.x = ("a"):byte(1)
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(eval_int(src), 0x61);
}

#[test]
fn several_targets_without_a_conflict_take_no_copies() {
    // `a.x, b.y = 10, 20`: neither `a` nor `b` is assigned by the
    // statement
    let src = r#"
        local a = { x = 0 }
        local b = { y = 0 }
        a.x, b.y = 10, 20
        return a.x, b.y
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "a@r0 copied");
    assert_eq!(count_moves_from_reg(&code, 1), 0, "b@r1 copied");
    assert_eq!(eval_int_pair(src), (10, 20));
}

#[test]
fn a_captured_table_is_not_copied() {
    // `bucket` is captured by a closure; the statement does not assign it
    let src = r#"
        local bucket = { x = 0 }
        local function _grab() return bucket end
        bucket.x = 99
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 0), 0, "bucket@r0 copied");
    assert_eq!(eval_int(src), 99);
}

#[test]
fn a_dotted_target_stores_into_the_inner_table() {
    // `outer.inner.x = 5`
    let src = r#"
        local outer = { inner = { x = 0 } }
        outer.inner.x = 5
        return outer.inner.x
    "#;
    assert_eq!(eval_int(src), 5);
}

#[test]
fn a_global_table_target_stores() {
    // `bucket.x = 7` with `bucket` a global
    let src = r#"
        bucket = { x = 0 }
        bucket.x = 7
        return bucket.x
    "#;
    assert_eq!(eval_int(src), 7);
}

#[test]
fn a_fresh_key_goes_through_newindex() {
    // SetField on an absent key calls `__newindex` with the stored value
    let src = r#"
        local fires = 0
        local seen_val = nil
        local bucket = { tokens = 100 }
        setmetatable(bucket, {
            __newindex = function(_, _, val)
                fires = fires + 1
                seen_val = val
            end,
        })
        bucket.fresh = bucket.tokens - 1
        if fires == 1 and seen_val == 99 then return 1 else return 0 end
    "#;
    assert_eq!(eval_int(src), 1);
}

#[test]
fn a_present_key_is_updated_without_newindex() {
    // SetField on a present key updates it; `__newindex` does not run
    let src = r#"
        local fires = 0
        local bucket = { tokens = 100 }
        setmetatable(bucket, {
            __newindex = function() fires = fires + 1 end,
        })
        bucket.tokens = bucket.tokens - 1
        if fires == 0 and bucket.tokens == 99 then return 1 else return 0 end
    "#;
    assert_eq!(eval_int(src), 1);
}

// ---------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------

/// `t[k] = v` with `k` a plain local: the key stays in its register, as
/// in PUC, instead of a copy into a temp.
#[test]
fn a_local_key_stays_in_its_register() {
    let src = "local t = {} local k = 3 local v = 5 t[k] = v return t[3]";
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 1), 0, "key copied: {code:?}");
    let set = code
        .iter()
        .find(|i| i.op() == Op::SetTable)
        .expect("SetTable");
    assert_eq!(
        set.b(),
        1,
        "SetTable should take the key from `k`: {code:?}"
    );
    assert_eq!(eval_int(src), 5);
}

/// A captured key is read from its variable when the store runs.
#[test]
fn a_captured_local_key_is_not_copied() {
    let src = "local t = {} local k = 3 local f = function() return k end t[k] = 1 return t[3]";
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 1),
        0,
        "captured key copied: {code:?}"
    );
    assert_eq!(eval_int(src), 1);
}

/// An unknown call on the right side does not copy the key.
#[test]
fn a_key_with_an_unknown_call_on_the_right_is_not_copied() {
    let src = "g = function() return 7 end local t = {} local k = 3 t[k] = g() return t[3]";
    let code = compile_main(src);
    assert_eq!(count_moves_from_reg(&code, 1), 0, "key copied: {code:?}");
    assert_eq!(eval_int(src), 7);
}

/// With several targets a later store may change the key: it is copied,
/// and `t[k], k = 10, 2` stores at the old `k`.
#[test]
fn a_key_in_a_multiple_assignment_is_copied() {
    let src = "local t = {} local k = 1 t[k], k = 10, 2 return t[1]";
    let code = compile_main(src);
    assert!(
        count_moves_from_reg(&code, 1) >= 1,
        "key not copied: {code:?}"
    );
    assert_eq!(eval_int(src), 10);
}

/// A call on the right that assigns the table's variable through an
/// upvalue: the store goes to the table the variable holds when the store
/// runs, as in PUC.
#[test]
fn a_call_that_rebinds_the_table_stores_into_the_new_one() {
    let src = "local b = {} local old = b \
               local function f() b = {} return 1 end \
               b.x = f() \
               if old.x == nil and b.x == 1 then return 1 else return 0 end";
    assert_eq!(eval_int(src), 1);
}
