//! v2.1 Phase 11 — A4' prerequisite tests for the RHS Call walker and the
//! AST-side metamethod-safety gate.
//!
//! These cover the helpers added in
//! `crates/luna-core/src/frontend/ast.rs`:
//!
//! - [`walk_rhs_for_calls`] — partitions reachable Call / MethodCall sites
//!   into `None / OnlyKnownPure / UserOrUnknown`.
//! - [`metamethod_safe_for_index_lhs`] — the bare-Name obj + safe-RHS
//!   gate the consumer (a future A4' attack) will compose with the
//!   `LocalVar.captured` check inside `Compiler`.
//!
//! Each test parses a one-statement Lua snippet (an assignment), reaches
//! into the chunk's AST for the relevant `ExprId`s, and asserts the
//! walker / gate verdict. The walker is exercised in isolation by reading
//! the RHS expression(s); the gate is exercised by feeding it
//! `(obj_eid, rhs_eid)` extracted from `Stat::Assign`.

use luna_core::frontend::ast::{
    Chunk, Expr, ExprId, RhsCallScan, Stat, metamethod_safe_for_index_lhs, walk_rhs_for_calls,
};
use luna_core::frontend::parser::parse;
use luna_core::version::LuaVersion;

// -----------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------

/// Parse a Lua snippet (Lua 5.5 dialect, luna's default) into a Chunk.
fn p(src: &str) -> Chunk {
    parse(src.as_bytes(), LuaVersion::Lua55).expect("parse should succeed")
}

/// Extract `(targets, exprs)` from the *last* top-level `Assign`
/// statement in the chunk; preceding `Local` / `local function` setup
/// stats are skipped.
fn assign_parts(chunk: &Chunk) -> (Vec<ExprId>, Vec<ExprId>) {
    for &sid in chunk.block.stats.iter().rev() {
        if let Stat::Assign { targets, exprs } = chunk.stat(sid) {
            return (targets.clone(), exprs.clone());
        }
    }
    panic!("no Assign statement in chunk");
}

/// Extract the obj/key ExprIds from a target whose root is `Expr::Index`.
fn index_obj_key(chunk: &Chunk, target: ExprId) -> (ExprId, ExprId) {
    match chunk.expr(target) {
        Expr::Index { obj, key } => (*obj, *key),
        other => panic!("expected Index target, got {other:?}"),
    }
}

// =================================================================
// Walker tests
// =================================================================

#[test]
fn walker_literal_rhs_is_none() {
    // `bucket.tokens = 1` — pure literal.
    let c = p("local bucket = {} bucket.tokens = 1");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::None);
}

#[test]
fn walker_local_var_rhs_is_none() {
    // `bucket.last = now` — pure local read.
    let c = p("local bucket = {} local now = 0 bucket.last = now");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::None);
}

#[test]
fn walker_binop_chain_no_call_is_none() {
    // `bucket.tokens = bucket.tokens - 1` — GetField + arith Sub.
    let c = p("local bucket = {} bucket.tokens = bucket.tokens - 1");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::None);
}

#[test]
fn walker_math_min_is_only_known_pure() {
    // The headline token_bucket RHS — math.min(...) lookup wins
    // the OnlyKnownPure tag.
    let c = p("local bucket = {tokens=0} bucket.tokens = math.min(1000, bucket.tokens + 10)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::OnlyKnownPure);
}

#[test]
fn walker_string_byte_is_only_known_pure() {
    let c = p("local t = {} t.v = string.byte('a', 1)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::OnlyKnownPure);
}

#[test]
fn walker_user_call_is_user_or_unknown() {
    // `t.v = f(1)` — unknown user-defined or global callee.
    let c = p("local t = {} local function f(x) return x end t.v = f(1)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::UserOrUnknown);
}

#[test]
fn walker_method_call_is_user_or_unknown() {
    // `obj:method(args)` cannot be proven pure — must reject.
    let c = p("local t = {} local s = 'hello' t.v = s:byte(1)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::UserOrUnknown);
}

#[test]
fn walker_aliased_local_is_user_or_unknown() {
    // `local m = math.min; t.v = m(1, 2)` — variable tracking is not
    // performed by the AST gate; the bare-Name callee `m` is unknown.
    let c = p("local m = math.min local t = {} t.v = m(1, 2)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::UserOrUnknown);
}

#[test]
fn walker_function_literal_rhs_is_none() {
    // Closure value flowing out is itself non-invocational at the
    // snapshot site. The body's Calls are NOT counted.
    let c = p("local t = {} t.v = function(x) return f(x) end");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::None);
}

#[test]
fn walker_nested_arith_with_known_pure_still_only_known_pure() {
    // math.min(1, 2 + math.min(3, 4) * 5) — nested known-pure stays
    // OnlyKnownPure across BinOp recursion.
    let c = p("local t = {} t.v = math.min(1, 2 + math.min(3, 4) * 5)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::OnlyKnownPure);
}

#[test]
fn walker_table_with_user_call_is_user_or_unknown() {
    // Table-constructor traversal must see the embedded user call.
    let c = p("local t = {} local function f() return 1 end t.v = { 1, 2, f() }");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::UserOrUnknown);
}

#[test]
fn walker_table_with_only_known_pure_is_only_known_pure() {
    let c = p("local t = {} t.v = { 1, math.min(2, 3), 'x' }");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::OnlyKnownPure);
}

#[test]
fn walker_paren_and_unop_preserve_pure() {
    let c = p("local t = {} t.v = -(1 + 2)");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::None);
}

#[test]
fn walker_os_clock_is_user_or_unknown() {
    // `os` is intentionally NOT on the known-pure root list — even
    // os.clock is conservatively treated as unknown for v2.1 ship.
    let c = p("local t = {} t.v = os.clock()");
    let (_, exprs) = assign_parts(&c);
    assert_eq!(walk_rhs_for_calls(&c, exprs[0]), RhsCallScan::UserOrUnknown);
}

// =================================================================
// Gate tests — metamethod_safe_for_index_lhs(chunk, obj, rhs)
// =================================================================

#[test]
fn gate_safe_for_local_obj_with_literal_rhs() {
    let c = p("local bucket = {} bucket.tokens = 42");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _key) = index_obj_key(&c, targets[0]);
    assert!(metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_safe_for_local_obj_with_math_min_rhs() {
    // The exact token_bucket pc 20 shape (RFC §2.2).
    let c = p("local bucket = {tokens=0} bucket.tokens = math.min(1000, bucket.tokens + 1)");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_rejects_user_call_rhs() {
    // RHS contains a user-defined function call — could re-bind
    // `bucket` via __newindex closure capture.
    let c = p("local bucket = {} local function f() return 1 end bucket.tokens = f()");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(!metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_rejects_method_call_rhs() {
    let c = p("local bucket = {} local s = 'a' bucket.tokens = s:byte(1)");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(!metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_rejects_non_name_obj_dotted_chain() {
    // `t.a.b = 1` — obj is itself `Index{ t, "a" }`, not a bare Name.
    // A4' v1 does not handle this case.
    let c = p("local t = {a={}} t.a.b = 1");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(!metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_safe_for_bracket_key_with_safe_rhs() {
    // `bucket[k] = math.min(...)` — bracketed Index with bare-Name obj
    // still passes the AST gate; the obj-is-Name property is what
    // matters.
    let c = p("local bucket = {} local k = 'x' bucket[k] = math.min(1, 2)");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_rejects_function_literal_rhs_via_obj_not_name() {
    // sanity check: when obj IS a Name and rhs is a function literal
    // (closure capture risk!), the AST gate accepts because the body
    // is not invoked at this site. Closure-capture modeling is a
    // documented conservative gap — flagged as accepted in the verdict.
    let c = p("local t = {} t.v = function() return f() end");
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    // Accepted as safe per the documented gap; if the gap closes in a
    // future hardening pass, this test should flip with a comment.
    assert!(metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}

#[test]
fn gate_rejects_user_call_chained_through_table_in_rhs() {
    // Edge case: user call deeply nested inside table constructor on
    // RHS — must propagate UserOrUnknown all the way out.
    let c = p(
        "local bucket = {} local function f() return 1 end bucket.tokens = (function() return f() end)()",
    );
    let (targets, exprs) = assign_parts(&c);
    let (obj, _) = index_obj_key(&c, targets[0]);
    assert!(!metamethod_safe_for_index_lhs(&c, obj, exprs[0]));
}
