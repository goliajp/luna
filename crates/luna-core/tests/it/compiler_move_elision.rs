//! Reloc-landing and RHS-materialization peephole regression tests.
//!
//! Reloc-landing peephole: when `assign_name` is about to emit a
//! `Move local_reg, vreg` for a local target, the just-emitted op at
//! `here() - 1` is inspected. If it is one of the closed set of
//! retargetable producers (arith / Get* / Unm / Len / Not / BNot /
//! GetUpval) whose A field equals `vreg` AND the pc is NOT a jump
//! destination, the A field is patched to `local_reg` directly via
//! `patch_dest` and the Move is skipped. Mirrors PUC `discharge2reg`.
//!
//! RHS-materialization elision: when `assign_stat`'s `explist_adjust` call ends with a
//! trivial `Move base, src` materialization (an `Exp::Reg(src)` RHS
//! discharged into a fresh temp) AND the single-store gate holds
//! (targets.len() == exprs.len() == 1), the Move is popped and `src`
//! is forwarded to the store as the value register, skipping the
//! materialization Move.
//!
//! Both peepholes are gated on `no_jump_lands_here`: a jump landing at the
//! modified instruction is fine, one landing after it keeps the Move.
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
    // In each `name = function ... end` assign_stat the Closure must land
    // on a temp (A >= 2), and a Move(name, temp) must follow.
    for c in &closures {
        assert!(
            c.a() >= 2,
            "Closure in `name = function...` assign_stat must write to a temp, \
             NOT retarget to the local (got A={})",
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

/// A jump landing right after the instruction (here: `and`'s short circuit
/// past the right operand) still needs the Move: that path skips it.
#[test]
fn jump_past_the_producer_keeps_the_move() {
    let src = "local x, a, b, c = 0, ... x = a and b + c return x";
    let code = compile_main(src);
    assert!(
        code.iter().any(|i| i.op() == Op::Move && i.a() == 0),
        "{code:?}"
    );
    for (a, want) in [("false", 0), ("1", 5)] {
        let src = format!("local x, a, b, c = 9, {a}, 2, 3 x = a and b + c return x or 0");
        assert_eq!(eval_int_all(&src), [want; 5], "{src}");
    }
}

// =====================================================================
// explist_adjust RHS materialization elision
// =====================================================================

#[test]
fn rhs_elision_single_target_simple_reg_rhs_elides_materialization() {
    // The token_bucket pc 29 shape — `bucket.last = now` where `now` is
    // a local. Without the elision the chunk emits `Move temp, now` +
    // `SetField bucket, c_last, temp`. With it, the Move drops and SetField
    // reads `now` directly.
    //
    // The proto carries two SetFields — one inside the table ctor for
    // `{ last = 0 }` and one for the assignment `bucket.last = now`.
    // The assignment's SetField is the second one in source order and
    // must read directly from `now@r1`.
    let src = r#"
        local bucket = { last = 0 }
        local now = 7
        bucket.last = now
        return bucket.last
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves(&code),
        0,
        "the RHS-materialization elision must elide the RHS materialization Move"
    );
    let setfields: Vec<&Inst> = code.iter().filter(|i| i.op() == Op::SetField).collect();
    assert_eq!(
        setfields.len(),
        2,
        "two SetFields: one inside `{{last=0}}` ctor, one for the assignment"
    );
    // The assignment's SetField (second in source order) must read
    // `now` directly: C = now@r1.
    assert_eq!(
        setfields[1].c(),
        1,
        "assignment SetField must read `now` directly (r1)"
    );
    assert_eq!(eval_table_get_int(src), 7);
}

#[test]
fn rhs_elision_name_target_simple_reg_rhs_collapses_to_one_move() {
    // `x = y` where both are locals. Without the elision explist_adjust
    // would emit Move(temp, y) and assign_name would emit Move(x, temp).
    // With it the materialization Move is popped, and only assign_name's
    // Move(x, y) remains.
    let src = r#"
        local x, y = 0, 41
        x = y
        return x
    "#;
    let code = compile_main(src);
    // Exactly one Move(x, y) survives.
    let moves: Vec<&Inst> = code.iter().filter(|i| i.op() == Op::Move).collect();
    assert_eq!(moves.len(), 1, "exactly one Move(x, y) survives");
    assert_eq!(moves[0].a(), 0, "Move target is x@r0");
    assert_eq!(moves[0].b(), 1, "Move source is y@r1");
    assert_eq!(eval_int(src), 41);
}

#[test]
fn rhs_elision_multi_target_preserves_materialization() {
    // Multi-target assignments cannot use the single-store short-circuit
    // (PUC §3.3.3 ordering). The materialization Moves stay.
    let src = r#"
        local a, b = 1, 2
        a, b = b, a
        return a * 10 + b
    "#;
    let code = compile_main(src);
    // Expect at least 2 Moves (one per materialization) plus possibly
    // the two store Moves — gate must NOT pop for multi-target.
    assert!(
        count_moves(&code) >= 2,
        "multi-target swap must keep at least the two materialization Moves"
    );
    // a, b = b, a -> a=2, b=1; 2*10 + 1 = 21.
    assert_eq!(eval_int(src), 21);
}

#[test]
fn rhs_elision_indexed_target_with_int_key_elides_materialization() {
    // `t[1] = x` short-circuit: same shape with SetI instead of SetField.
    let src = r#"
        local t = { 0 }
        local x = 99
        t[1] = x
        return t[1]
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves(&code),
        0,
        "the RHS-materialization elision must elide materialization Move for SetI store"
    );
    // SetI with C == x's local register.
    let seti: Vec<&Inst> = code.iter().filter(|i| i.op() == Op::SetI).collect();
    assert_eq!(seti.len(), 1, "exactly one SetI");
    assert_eq!(seti[0].c(), 1, "SetI reads x directly (r1)");
    assert_eq!(eval_table_get_int(src), 99);
}

// =====================================================================
// Bundle interaction — both peepholes active on the same statement
// =====================================================================

#[test]
fn bundle_token_bucket_repeated_pattern() {
    // Compresses the token_bucket inner-loop shape: refill +1, last =
    // now, tokens -1. Each statement targets a different peephole; the
    // bundle clears 3 Moves per iter (Reloc-landing twice + RHS
    // materialization once).
    let src = r#"
        local bucket = { tokens = 1000, last = 0 }
        local now = 1
        local refilled = 0
        for i = 1, 10 do
            refilled = refilled + 1
            bucket.last = now
            bucket.tokens = bucket.tokens - 1
            now = now + 1
        end
        return refilled + bucket.tokens
    "#;
    // refilled = 10, bucket.tokens = 990, total = 1000.
    assert_eq!(eval_int(src), 1000);
}

#[test]
fn bundle_correctness_cross_check_against_baseline_observation() {
    // Differential check: a hand-coded golden value against the bundled
    // compiler. If either peephole had introduced a semantic drift, this
    // tight numeric expression would diverge.
    let src = r#"
        local a, b, c = 3, 5, 7
        local x = 0
        x = a + b * c
        local t = { x = 0 }
        t.x = x
        return t.x
    "#;
    assert_eq!(eval_int(src), 3 + 5 * 7);
}

/// `b = a` right after `a = a + 1`: the store takes `a`'s own register, so
/// the instruction before it is the `Add` of the previous statement, whose
/// destination is `a` itself; retargeting it to `b` dropped the increment.
#[test]
fn store_from_a_local_keeps_the_previous_assignment_to_it() {
    assert_eq!(
        eval_int("local b local a = 0 a = a + 1 b = a a = a + 1 return a * 10 + b"),
        21
    );
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        vm.open_base();
        let src = "local b local a = 0 while a < 3 do a = a + 1 b = a end return b";
        let r = vm
            .eval(src)
            .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_text(&e)));
        assert!(
            matches!(r.first(), Some(Value::Int(3)))
                || matches!(r.first(), Some(Value::Float(f)) if *f == 3.0),
            "{v:?}: {r:?}"
        );
    }
}
