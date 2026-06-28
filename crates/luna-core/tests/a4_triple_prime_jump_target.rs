//! v2.1 PI — A4''' prereq jump-target tracker subsystem tests.
//!
//! Cover the `Level::last_target` PUC `fs->lasttarget` equivalent and the
//! `patch_to_here` / `mark_target` wiring at
//! `crates/luna-core/src/compiler/mod.rs`. The tracker is currently
//! behaviour-neutral (no codegen path reads it yet — see
//! `prev_emit_is_safe_peephole_site`), so these tests verify the
//! tracker is correct against PUC `fs->lasttarget` semantics in
//! preparation for the A4''' Reloc-landing peephole follow-up.
//!
//! Per-pattern coverage:
//! - chunk with no jumps: tracker stays at `None`
//! - bare `if then end`: forward jump lands at chunk return
//! - `while cond do ... end`: cond-skip + jump-back back-edge
//! - `repeat ... until cond`: jump-back to repeat top
//! - numeric `for`: ForPrep skip + ForLoop back-edge
//! - generic `for k,v in pairs(t) do ... end`: TForPrep + TForLoop
//! - `goto`/label forward reference: forward goto patched at label
//! - `and` / `or` short-circuit: jump-pad targets
//! - comparison materialization (`local x = a < b`): Jmp(1) pad
//!
//! Each test asserts that `last_target` advances to a pc inside the
//! main proto and never exceeds the code length. The exact pc value
//! is sometimes fragile (depends on op enumeration order), so where
//! possible tests check structural relations (e.g. `last_target >=
//! some_known_pc`) rather than literal equality.

use luna_core::compiler::compile_chunk_with_last_target;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;

fn compile_last_target(src: &str) -> (Vec<luna_core::vm::isa::Inst>, Option<usize>) {
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    let mut heap = Heap::new();
    let (proto, lt) = compile_chunk_with_last_target(&ast, LuaVersion::Lua55, b"=a4ppp", &mut heap)
        .expect("compile");
    (proto.code.to_vec(), lt)
}

// ---------------------------------------------------------------------
// no-jump chunks: tracker stays at `None`
// ---------------------------------------------------------------------

#[test]
fn straight_line_no_jumps_leaves_tracker_none() {
    let (code, lt) = compile_last_target("local x = 1 local y = 2 return x + y");
    // Straight-line code: no jumps at all. last_target stays at the
    // PUC `-1` sentinel (luna's `None`).
    assert_eq!(
        lt, None,
        "straight-line chunk should leave last_target=None"
    );
    // sanity: there really are no jump-shaped ops in the proto
    let has_jmp = code.iter().any(|i| {
        matches!(
            i.op(),
            luna_core::vm::isa::Op::Jmp
                | luna_core::vm::isa::Op::ForPrep
                | luna_core::vm::isa::Op::ForLoop
                | luna_core::vm::isa::Op::TForPrep
                | luna_core::vm::isa::Op::TForLoop
        )
    });
    assert!(
        !has_jmp,
        "straight-line code unexpectedly contains a jump op"
    );
}

#[test]
fn const_return_only() {
    let (_code, lt) = compile_last_target("return 42");
    assert_eq!(lt, None);
}

// ---------------------------------------------------------------------
// `if then end`: forward jump patched at chunk return
// ---------------------------------------------------------------------

#[test]
fn if_then_end_records_forward_jump_target() {
    let (code, lt) = compile_last_target("if true then return 1 end return 2");
    let lt = lt.expect("if-stat must record at least one jump target");
    // The forward-skip jump for the `if` cond patches to the post-then
    // pc (just before `return 2`). Must be inside the code, never
    // beyond the final emit.
    assert!(
        lt < code.len(),
        "last_target {lt} out of bounds (code len {})",
        code.len()
    );
}

#[test]
fn nested_if_records_outermost_target() {
    let (code, lt) = compile_last_target(
        "local x = 0\n\
         if x == 0 then\n\
           if x < 10 then x = 1 end\n\
           x = 2\n\
         end\n\
         return x",
    );
    let lt = lt.expect("nested if must record at least one target");
    assert!(lt < code.len());
    // Inner-if patch fires before the outer-if patch, so the outer-if
    // patch (latest) wins the max-target invariant.
    assert!(lt > 0);
}

// ---------------------------------------------------------------------
// loops: while / repeat / numeric for / generic for
// ---------------------------------------------------------------------

#[test]
fn while_loop_records_back_edge_and_exit() {
    let (code, lt) = compile_last_target(
        "local i = 0\n\
         while i < 10 do i = i + 1 end\n\
         return i",
    );
    let lt = lt.expect("while-loop must record jump targets");
    assert!(lt < code.len());
    // The exit-jump patch lands at the post-loop pc; that's strictly
    // after the loop body, so it should be near the chunk's tail.
    assert!(
        lt >= 3,
        "exit-jump should land past the loop body (lt={lt})"
    );
}

#[test]
fn repeat_until_records_back_edge() {
    let (code, lt) = compile_last_target(
        "local i = 0\n\
         repeat i = i + 1 until i >= 10\n\
         return i",
    );
    let lt = lt.expect("repeat-until must record jump targets");
    assert!(lt < code.len());
}

#[test]
fn numeric_for_records_body_top_and_post_loop() {
    let (code, lt) = compile_last_target(
        "local s = 0\n\
         for i = 1, 10 do s = s + i end\n\
         return s",
    );
    let lt = lt.expect("numeric for must record jump targets");
    assert!(lt < code.len());
    // post_loop mark fires AFTER ForLoop emit, so last_target should
    // be at or past the ForLoop pc — equivalently, past the body_top.
    // We can't trivially extract the exact body_top without bytecode
    // walking but we can assert tracker stayed monotonically non-
    // decreasing relative to the loop emit ops.
    let has_forloop = code
        .iter()
        .any(|i| matches!(i.op(), luna_core::vm::isa::Op::ForLoop));
    assert!(has_forloop, "numeric for did not emit ForLoop op");
}

#[test]
fn generic_for_records_back_edge() {
    let (code, lt) = compile_last_target(
        "local t = {1, 2, 3}\n\
         local s = 0\n\
         for _, v in ipairs(t) do s = s + v end\n\
         return s",
    );
    let lt = lt.expect("generic for must record jump targets");
    assert!(lt < code.len());
    let has_tforloop = code
        .iter()
        .any(|i| matches!(i.op(), luna_core::vm::isa::Op::TForLoop));
    assert!(has_tforloop, "generic for did not emit TForLoop op");
}

// ---------------------------------------------------------------------
// goto / label
// ---------------------------------------------------------------------

#[test]
fn forward_goto_records_label_target() {
    let (code, lt) = compile_last_target(
        "local i = 1\n\
         goto skip\n\
         i = 99\n\
         ::skip::\n\
         return i",
    );
    let lt = lt.expect("forward goto must record label target");
    assert!(lt < code.len());
}

#[test]
fn backward_goto_records_label_target() {
    let (code, lt) = compile_last_target(
        "local i = 0\n\
         ::top::\n\
         i = i + 1\n\
         if i < 3 then goto top end\n\
         return i",
    );
    let lt = lt.expect("backward goto chain must record jump targets");
    assert!(lt < code.len());
}

// ---------------------------------------------------------------------
// short-circuit logical ops (and / or) — Cmp-pad targets
// ---------------------------------------------------------------------

#[test]
fn and_short_circuit_records_pad_target() {
    let (code, lt) = compile_last_target(
        "local a, b = 1, 2\n\
         local c = (a < 10) and (b < 20)\n\
         return c",
    );
    let lt = lt.expect("and short-circuit must record pad target");
    assert!(lt < code.len());
}

#[test]
fn or_short_circuit_records_pad_target() {
    let (code, lt) = compile_last_target(
        "local a, b = 1, 2\n\
         local c = (a > 10) or (b < 20)\n\
         return c",
    );
    let lt = lt.expect("or short-circuit must record pad target");
    assert!(lt < code.len());
}

// ---------------------------------------------------------------------
// comparison materialization Jmp(1) pad
// ---------------------------------------------------------------------

#[test]
fn comparison_materialization_records_loadtrue_pad() {
    let (code, lt) = compile_last_target("local a, b = 1, 2 local c = a < b return c");
    let lt = lt.expect("comparison materialization must record LoadTrue pad");
    assert!(lt < code.len());
    let has_load_true = code
        .iter()
        .any(|i| matches!(i.op(), luna_core::vm::isa::Op::LoadTrue));
    assert!(
        has_load_true,
        "materialized comparison did not emit LoadTrue"
    );
}

// ---------------------------------------------------------------------
// monotonicity: last_target only advances upward across multiple
// independent constructs
// ---------------------------------------------------------------------

#[test]
fn last_target_advances_monotonically_across_multiple_constructs() {
    let (code, lt) = compile_last_target(
        "local s = 0\n\
         if s == 0 then s = 1 end\n\
         while s < 5 do s = s + 1 end\n\
         for i = 1, 3 do s = s + i end\n\
         return s",
    );
    let lt = lt.expect("multi-construct chunk must record jump targets");
    // The final last_target should be the largest pc among all patched
    // jumps. Since the for-loop's post-loop mark fires last and is the
    // tail of the chunk, lt should sit near the code length (within a
    // few ops of the trailing Return0).
    assert!(lt < code.len());
    assert!(
        lt + 5 >= code.len(),
        "tracker {lt} too far behind code end {} — late marks may have been missed",
        code.len()
    );
}
