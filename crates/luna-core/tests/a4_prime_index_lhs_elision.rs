//! v2.1 Phase 11 — A4' attack regression tests.
//!
//! Cover the Index-LHS object-snapshot Move elision wired at
//! `crates/luna-core/src/compiler/mod.rs` `assign_stat` Index-LHS branch
//! (line 2487 region) via the prereq gate
//! [`Compiler::assign_stat_can_skip_obj_snapshot`].
//!
//! Each test compiles a focused snippet via the public `compile_chunk`
//! entry, inspects the main proto's bytecode for the specific
//! `Op::Move` shape we care about, and evaluates the same source under
//! `Vm` to cross-check that the elision did not change observable
//! semantics (no silent behaviour change).
//!
//! The Move count assertions discriminate A4' (the obj snapshot Move,
//! whose source is the local-bucket register) from A4'' (the RHS
//! materialization Move whose source is the *RHS* local reg, deferred
//! to a later v2.1 ship). Helper `count_moves_from_reg(src, b)` counts
//! only Moves whose source operand is `b`, isolating the A4' decision
//! from A4'' residual noise.

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
    let proto = compile_chunk(&ast, LuaVersion::Lua55, b"=a4_prime", &mut heap).expect("compile");
    proto.code.to_vec()
}

/// Count `Op::Move` ops in the main proto whose source-register
/// operand `b` equals `src_reg`. The A4' snapshot Move always has
/// `b == obj_local_reg`.
fn count_moves_from_reg(code: &[Inst], src_reg: u32) -> usize {
    code.iter()
        .filter(|i| matches!(i.op(), Op::Move) && i.b() == src_reg)
        .count()
}

/// Count all `Op::Move` ops (used where we want the total instead of
/// the A4'-specific source-reg filter).
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
// Safe path — A4' snapshot Move elided
// =====================================================================

#[test]
fn safe_bucket_self_decrement_elides_snapshot() {
    // `bucket.tokens = bucket.tokens - 1` — RHS is a Sub reloc whose
    // result lands directly into the result reg, so the only Move
    // candidate would be the obj snapshot. Gate accepts → 0 Moves.
    let src = r#"
        local bucket = { tokens = 100 }
        bucket.tokens = bucket.tokens - 1
        return bucket.tokens
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        0,
        "safe Index-LHS snapshot Move (src=bucket@r0) was not elided"
    );
    assert_eq!(count_all_moves(&code), 0, "no other Moves expected either");
    assert_eq!(eval_int(src), 99);
}

#[test]
fn safe_math_min_rhs_elides_snapshot() {
    // Headline token_bucket pc 20 shape: math.min(...) is the
    // OnlyKnownPure RHS class. Gate accepts → bucket@r0 has no
    // snapshot Move. (The Call op's self-arg-shuffle Move appears
    // with src != 0 if it appears at all; not under the A4' filter.)
    let src = r#"
        local bucket = { tokens = 0 }
        bucket.tokens = math.min(1000, bucket.tokens + 10)
        return bucket.tokens
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        0,
        "math.min RHS path must elide the bucket@r0 snapshot Move"
    );
    assert_eq!(eval_int(src), 10);
}

#[test]
fn safe_literal_local_rhs_elides_a4prime_snapshot_only() {
    // `bucket.last = now` (now is a local reg). A4' elides the obj
    // snapshot (no Move with src=bucket@r0). With the A4'' bundle
    // shipped (see `.dev/rfcs/v2.1-a4-triple-double-bundle-verdict.md`),
    // the RHS local force-materialization Move (src=now@r1) is ALSO
    // elided — the SetField now reads `now` directly. Both halves
    // assert zero Moves; the SetField uses `now@r1` directly as its
    // C operand (cross-verified by `eval_int`).
    let src = r#"
        local bucket = { last = 0 }
        local now = 7
        bucket.last = now
        return bucket.last
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        0,
        "bucket@r0 snapshot Move must be elided"
    );
    assert_eq!(
        count_moves_from_reg(&code, 1),
        0,
        "A4'' RHS materialization Move (src=now@r1) is now elided by the bundle"
    );
    assert_eq!(eval_int(src), 7);
}

// =====================================================================
// Unsafe paths — A4' snapshot Move preserved
// =====================================================================

#[test]
fn unsafe_user_call_rhs_preserves_snapshot() {
    // `bucket.x = f()` — UserOrUnknown RHS. Gate rejects so a captured
    // upvalue inside f could rebind bucket while the RHS evaluates.
    // The bucket@r0 snapshot Move stays.
    let src = r#"
        local bucket = { x = 1 }
        local function f() return 42 end
        bucket.x = f()
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        1,
        "user-call RHS must keep the bucket@r0 snapshot Move"
    );
    assert_eq!(eval_int(src), 42);
}

#[test]
fn unsafe_method_call_rhs_preserves_snapshot() {
    // `bucket.x = ("a"):byte(1)` — MethodCall RHS is unconditionally
    // UserOrUnknown per the walker doc.
    let src = r#"
        local bucket = { x = 0 }
        bucket.x = ("a"):byte(1)
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        1,
        "method-call RHS must keep the bucket@r0 snapshot Move"
    );
    assert_eq!(eval_int(src), 0x61);
}

#[test]
fn unsafe_multi_target_preserves_snapshots() {
    // PUC §3.3.3 multi-target ordering: every Index-LHS obj must be
    // snapshotted before any store. Gate rejects multi-target →
    // both bucket@r0 and bucket@r1 snapshot Moves stay.
    let src = r#"
        local a = { x = 0 }
        local b = { y = 0 }
        a.x, b.y = 10, 20
        return a.x, b.y
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        1,
        "multi-target a@r0 snapshot Move must remain"
    );
    assert_eq!(
        count_moves_from_reg(&code, 1),
        1,
        "multi-target b@r1 snapshot Move must remain"
    );
    assert_eq!(eval_int_pair(src), (10, 20));
}

#[test]
fn unsafe_captured_obj_preserves_snapshot() {
    // `bucket` is captured by an inner closure. Even though this
    // snippet's RHS is a pure literal, a __newindex-stored closure
    // could re-bind bucket through the upvalue — the conservative
    // reject keeps the snapshot for the captured-local owner.
    let src = r#"
        local bucket = { x = 0 }
        local function _grab() return bucket end
        bucket.x = 99
        return bucket.x
    "#;
    let code = compile_main(src);
    assert_eq!(
        count_moves_from_reg(&code, 0),
        1,
        "captured-local obj must keep the snapshot Move"
    );
    assert_eq!(eval_int(src), 99);
}

#[test]
fn unsafe_dotted_chain_lhs_falls_back_correctly() {
    // `outer.inner.x = 5` — obj is `outer.inner`, not a bare Name.
    // Gate rejects per RFC §5 conservative gap 4. The obj snapshot
    // is structurally a GetField Reloc patch (not a Move op), so
    // we only check semantic equivalence here.
    let src = r#"
        local outer = { inner = { x = 0 } }
        outer.inner.x = 5
        return outer.inner.x
    "#;
    assert_eq!(eval_int(src), 5);
}

#[test]
fn unsafe_global_obj_falls_back_correctly() {
    // Global obj — obj is GetTabUp Reloc, not Reg(local). Gate
    // rejects on "obj is not a current-level local". Same as the
    // dotted-chain case, the snapshot is a Reloc patch not a Move.
    let src = r#"
        bucket = { x = 0 }
        bucket.x = 7
        return bucket.x
    "#;
    assert_eq!(eval_int(src), 7);
}

// =====================================================================
// Cross-path semantic equivalence under metamethod observability
// =====================================================================

#[test]
fn elision_preserves_newindex_semantics_on_fresh_key() {
    // Safe-path RHS (pure arith → gate accepts) elides the snapshot,
    // then SetField fires __newindex because the key is absent on the
    // bucket. The metamethod must observe the same receiver / value
    // as in the snapshot-kept path.
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
fn elision_preserves_present_key_in_place_update() {
    // Present-key SetField under safe path: bucket.tokens is already
    // present → in-place update via try_set_existing collapse;
    // __newindex must NOT fire. The A4' elision composes with A3 +
    // try_set_existing single-walk semantics.
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
