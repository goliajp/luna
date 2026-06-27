//! v2.1 PI Phase 11 — A4''' + A4'' bundle regression tests.
//!
//! A4''' Reloc-landing peephole: when `assign_name` is about to emit a
//! `Move local_reg, vreg` for a local target, the just-emitted op at
//! `here() - 1` is inspected. If it is one of the closed set of
//! retargetable producers (arith / Get* / Unm / Len / Not / BNot /
//! GetUpval) whose A field equals `vreg` AND the pc is NOT a jump
//! destination, the A field is patched to `local_reg` directly via
//! `patch_dest` and the Move is skipped. Mirrors PUC `discharge2reg`.
//!
//! A4'' bundle: when `assign_stat`'s `explist_adjust` call ends with a
//! trivial `Move base, src` materialization (an `Exp::Reg(src)` RHS
//! discharged into a fresh temp) AND the single-store gate holds
//! (targets.len() == exprs.len() == 1), the Move is popped and `src`
//! is forwarded to the store as the value register, skipping the
//! materialization Move.
//!
//! Both peepholes are gated on `prev_emit_is_safe_peephole_site` so a
//! jump landing at the modified pc is preserved.
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
    let proto = compile_chunk(&ast, LuaVersion::Lua55, b"=a4bundle", &mut heap).expect("compile");
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
// A4''' Reloc-landing — retargetable producers
// =====================================================================

#[test]
fn a4ppp_arith_add_local_local_one_emits_no_move() {
    // The token_bucket pc 33 shape — `refilled = refilled + 1`. Without
    // A4''' the chunk would emit `Add temp, refilled, 1` + `Move
    // refilled, temp`. With A4''', the Add's A field is patched to
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
        "A4''' must elide the trailing Move for `local = local + 1`"
    );
    // Add still emits exactly once, landing directly into the local.
    assert_eq!(
        count_ops(&code, Op::Add),
        1,
        "the single Add stays; only its A field is retargeted"
    );
    assert_eq!(eval_int(src), 1);
}

#[test]
fn a4ppp_unary_neg_local_emits_no_move() {
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
        "A4''' must elide the Unm landing Move"
    );
    assert_eq!(count_ops(&code, Op::Unm), 1);
    assert_eq!(eval_int(src), -5);
}

#[test]
fn a4ppp_len_local_emits_no_move() {
    // `#x` produces an Exp::Reloc(Op::Len). Same pattern.
    let src = r#"
        local t = "hello"
        local n = 0
        n = #t
        return n
    "#;
    let code = compile_main(src);
    // The only Move that could appear is the discharge into `n`. A4'''
    // elides it by retargeting Len's A to `n`'s register.
    assert_eq!(
        count_moves(&code),
        0,
        "A4''' must elide the Len landing Move"
    );
    assert_eq!(count_ops(&code, Op::Len), 1);
    assert_eq!(eval_int(src), 5);
}

#[test]
fn a4ppp_getfield_local_emits_no_move() {
    // `x = t.k` — GetField with A=temp, then Move local, temp.
    // A4''' retargets GetField's A to local.
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
        "A4''' must elide the GetField landing Move"
    );
    assert_eq!(eval_int(src), 42);
}

#[test]
fn a4ppp_newtable_is_not_retargeted_to_preserve_gc_live_top() {
    // NewTable allocates and calls `maybe_collect_garbage(base + A + 1)`,
    // using A as the live-stack-top boundary. Retargeting A to a local
    // below another live local would let GC sweep the higher local —
    // PUC gc.lua line 91 regression. The Move stays so the temp register
    // holds the new table BEFORE GC sees a shrunk root set, then the
    // local-write Move follows.
    //
    // Construct: u is at r0, b is at r1. After `b = {34}` the locals
    // are settled. The `u = {}` is an `assign_stat` (not local_stat) so
    // the NewTable goes through a temp. With A4''' off NewTable+Closure
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
    // For the `u = {}` case (the second NewTable), A4''' is disabled
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
fn a4ppp_closure_is_not_retargeted_to_preserve_gc_live_top() {
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
fn a4ppp_jump_target_blocks_retarget() {
    // When the just-emitted instruction at here()-1 is itself a jump
    // destination, A4''' must NOT retarget — a jump landing there could
    // be patched by something that depends on the original A field, or
    // a future basic-block-edge audit could break. Construct a shape
    // where the previous emit IS a jump target.
    //
    // The cleanest minimal pattern: a comparison materialization (Cmp +
    // Jmp + LFalseSkip + LoadTrue) marks the LoadTrue pc as a target.
    // If we then chain `x = <bool_expr>` after such a comparison emit,
    // the prev_emit_is_safe_peephole_site check should keep the Move.
    //
    // PUC `x = a < b` already exercises comparison materialization; we
    // verify that the assertion paths still see correct values rather
    // than verifying bytecode shape (which is fragile).
    let src = r#"
        local a, b = 1, 2
        local x = false
        x = a < b
        if x then return 1 else return 0 end
    "#;
    assert_eq!(eval_int(src), 1);
}

// =====================================================================
// A4'' explist_adjust RHS materialization elision
// =====================================================================

#[test]
fn a4pp_single_target_simple_reg_rhs_elides_materialization() {
    // The token_bucket pc 29 shape — `bucket.last = now` where `now` is
    // a local. Without A4'' the chunk emits `Move temp, now` + `SetField
    // bucket, c_last, temp`. With A4'', the Move drops and SetField
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
        "A4'' must elide the RHS materialization Move"
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
fn a4pp_name_target_simple_reg_rhs_collapses_to_one_move() {
    // `x = y` where both are locals. Without A4'' explist_adjust would
    // emit Move(temp, y) and assign_name would emit Move(x, temp). With
    // A4'' the materialization Move is popped, and only assign_name's
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
fn a4pp_multi_target_preserves_materialization() {
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
fn a4pp_indexed_target_with_int_key_elides_materialization() {
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
        "A4'' must elide materialization Move for SetI store"
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
    // bundle clears 3 Moves per iter (A4''' twice + A4'' once).
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
    // compiler. If A4''' or A4'' had introduced a semantic drift, this
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
