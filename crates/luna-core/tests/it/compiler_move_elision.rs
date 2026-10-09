//! An assignment whose value count matches its targets stores the last
//! value straight into the last target (PUC `restassign` then
//! `luaK_storevar`): an instruction whose destination is still open writes
//! the local directly, and a value already in a register is moved once.
//!
//! Each test compiles a focused snippet, inspects the main proto's
//! bytecode for the expected shape, and cross-checks observable
//! semantics by running it under `Vm`.

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
    let proto =
        compile_chunk(&ast, LuaVersion::Lua55, b"=move_elision", &mut heap).expect("compile");
    proto.code.to_vec()
}

fn count_moves(code: &[Inst]) -> usize {
    code.iter().filter(|i| matches!(i.op(), Op::Move)).count()
}

fn count_ops(code: &[Inst], op: Op) -> usize {
    code.iter().filter(|i| i.op() == op).count()
}

fn eval_int(src: &str) -> i64 {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.open_base();
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

fn eval_table_get_int(src: &str) -> i64 {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.open_base();
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

// =====================================================================
// Reloc-landing — retargetable producers
// =====================================================================

#[test]
fn retarget_arith_add_local_local_one_emits_no_move() {
    // The token_bucket pc 33 shape — `refilled = refilled + 1`. Without
    // the peephole the chunk would emit `Add temp, refilled, 1` + `Move
    // refilled, temp`. With it, the Add's A field is patched to
    // refilled directly and the Move drops.
    let src = r#"
        local refilled = 0
        refilled = refilled + 1
        return refilled
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves(&code),
        0,
        "the Reloc-landing peephole must elide the trailing Move for `local = local + 1`"
    );
    // the add still emits exactly once, landing directly into the local
    assert_eq!(
        count_ops(&code, Op::AddI),
        1,
        "the single AddI stays; only its A field is retargeted"
    );
    assert_eq!(eval_int(src), 1);
}

#[test]
fn retarget_unary_neg_local_emits_no_move() {
    // Unary Unm produces Exp::Reloc whose A is patched at discharge.
    // The Move from Unm-temp to the local should be elided.
    let src = r#"
        local x = 5
        x = -x
        return x
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves(&code),
        0,
        "the Reloc-landing peephole must elide the Unm landing Move"
    );
    assert_eq!(count_ops(&code, Op::Unm), 1);
    assert_eq!(eval_int(src), -5);
}

#[test]
fn retarget_len_local_emits_no_move() {
    // `#x` produces an Exp::Reloc(Op::Len). Same pattern.
    let src = r#"
        local t = "hello"
        local n = 0
        n = #t
        return n
    "#;
    let code = compile_main(src);
    // The only Move that could appear is the discharge into `n`. The
    // peephole elides it by retargeting Len's A to `n`'s register.
    assert_eq!(
        count_moves(&code),
        0,
        "the Reloc-landing peephole must elide the Len landing Move"
    );
    assert_eq!(count_ops(&code, Op::Len), 1);
    assert_eq!(eval_int(src), 5);
}

#[test]
fn retarget_getfield_local_emits_no_move() {
    // `x = t.k` — GetField with A=temp, then Move local, temp.
    // The peephole retargets GetField's A to local.
    let src = r#"
        local t = { k = 42 }
        local x = 0
        x = t.k
        return x
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves(&code),
        0,
        "the Reloc-landing peephole must elide the GetField landing Move"
    );
    assert_eq!(eval_int(src), 42);
}

#[test]
fn retarget_newtable_is_not_retargeted_to_preserve_gc_live_top() {
    // NewTable allocates and calls `maybe_collect_garbage(base + A + 1)`,
    // using A as the live-stack-top boundary. Retargeting A to a local
    // below another live local would let GC sweep the higher local —
    // PUC gc.lua line 91 regression. The Move stays so the temp register
    // holds the new table BEFORE GC sees a shrunk root set, then the
    // local-write Move follows.
    //
    // Construct: u is at r0, b is at r1. After `b = {34}` the locals
    // are settled. The `u = {}` is an `assign_stat` (not local_stat) so
    // the NewTable goes through a temp. With the peephole off NewTable+Closure
    // the temp must be a fresh reg (>= 3 here = above all locals), then
    // Move(0, temp) writes it to u.
    let src = r#"
        local u
        local b
        b = { 34 }
        u = {}
        return b[1]
    "#;
    let code = compile_main(src);
    let new_tables: Vec<(usize, &Inst)> = code
        .iter()
        .enumerate()
        .filter(|(_, i)| i.op() == Op::NewTable)
        .collect();
    assert_eq!(
        new_tables.len(),
        2,
        "expected two NewTable ops (one for `b = {{34}}`, one for `u = {{}}`)"
    );
    // For the `u = {}` case (the second NewTable), the peephole is disabled
    // for NewTable so it must write to a temp at freereg (>= 2), NOT to
    // u's r0 directly.
    let (_, second_newtable) = new_tables[1];
    assert!(
        second_newtable.a() >= 2,
        "second NewTable (`u = {{}}`) must write to a temp, \
         NOT retarget to u@r0 (got A={})",
        second_newtable.a()
    );
    // And a Move must follow it that copies the table into u@r0.
    let moves_to_u: usize = code
        .iter()
        .filter(|i| i.op() == Op::Move && i.a() == 0 && i.b() == second_newtable.a())
        .count();
    assert!(
        moves_to_u >= 1,
        "expected a Move(u@r0, temp) following the second NewTable"
    );
    assert_eq!(eval_int(src), 34);
}

#[test]
fn retarget_closure_is_not_retargeted_to_preserve_gc_live_top() {
    // Closure shares NewTable's GC-step-with-A-derived-live-top contract.
    // Same exclusion. Force the assignment shape (not local_stat) so
    // discharge would normally land on a temp.
    let src = r#"
        local f
        local g
        f = function() return 2 end
        g = function() return 1 end
        return f() + g()
    "#;
    let code = compile_main(src);
    let closures: Vec<&Inst> = code.iter().filter(|i| i.op() == Op::Closure).collect();
    assert_eq!(closures.len(), 2, "two Closure ops expected");
    // 5.2+ put a closure in the next free register at once (PUC
    // `codeclosure`): it lands on a temp (A >= 2) and is moved.
    for c in &closures {
        assert!(
            c.a() >= 2,
            "Closure in `name = function...` must write to a temp (got A={})",
            c.a()
        );
    }
    assert_eq!(eval_int(src), 3);
}

#[test]
fn retarget_comparison_value_keeps_its_pads() {
    // `x = a < b` materializes through `LFalseSkip` / `LoadTrue`, both
    // jump destinations writing the temporary; neither is retargetable, so
    // the Move into `x` stays.
    let src = r#"
        local a, b = 1, 2
        local x = false
        x = a < b
        if x then return 1 else return 0 end
    "#;
    assert_eq!(eval_int(src), 1);
}

/// All five dialects: the value `src` returns, as an integer.
fn eval_int_all(src: &str) -> Vec<i64> {
    [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ]
    .into_iter()
    .map(|v| {
        let mut vm = Vm::new(v);
        vm.open_base();
        let r = vm
            .eval(src)
            .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_text(&e)));
        match r.first() {
            Some(Value::Int(i)) => *i,
            Some(Value::Float(f)) => *f as i64,
            other => panic!("{v:?}: {other:?}"),
        }
    })
    .collect()
}

/// The statement after an `if ... end` starts at the `if`'s skip target;
/// PUC still writes its result straight into the local (`discharge2reg`
/// does not look at `fs->lasttarget`).
#[test]
fn retarget_at_a_jump_target_after_if() {
    let src = "local n, c = 0, ... if c then n = n + 5 end n = n + 1 return n";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 0, "{code:?}");
    assert!(
        code.iter()
            .filter(|i| i.op() == Op::AddI)
            .all(|i| i.a() == 0)
    );
    for (c, want) in [("true", 6), ("false", 1)] {
        let src = format!("local n, c = 0, {c} if c then n = n + 5 end n = n + 1 return n");
        assert_eq!(eval_int_all(&src), [want; 5], "{src}");
    }
}

/// `k = #t` right after an `if ... end`: the `Len` lands in `k`.
#[test]
fn retarget_len_at_a_jump_target() {
    let src = "local t, k = {1, 2, 3}, 0 if k == 0 then k = k + 1 end k = #t return k";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 0, "{code:?}");
    assert_eq!(eval_int_all(src), [3; 5]);
}

/// `w = v` right after a numeric `for`: the loop's exit lands on the
/// temporary Move, which is dropped; the store reads `v` directly.
#[test]
fn rhs_elision_at_a_loop_exit() {
    let src = "local w, v = 0, 7 for i = 1, 3 do v = v + i end w = v return w * 100 + v";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 1, "{code:?}");
    assert_eq!(eval_int_all(src), [1313; 5]);
}

mod rhs_elision;
mod testset;
