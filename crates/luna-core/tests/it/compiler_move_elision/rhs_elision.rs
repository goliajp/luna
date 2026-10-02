//! explist_adjust RHS materialization elision, and both peepholes active on the same statement.

use super::*;

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
