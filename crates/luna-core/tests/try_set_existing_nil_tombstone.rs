//! Regression tests for the `Table::clear_existing_slot` tombstone
//! discipline shared between `set_norm` and `try_set_existing`.
//!
//! Backstory: `try_set_existing` (introduced by A3 — newindex
//! single-walk collapse, commit `be77811`) and `set_norm` are parallel
//! write entry points into `Table`. They must agree on the
//! "live with val=Nil is illegal" state. On the C3 Session 2 SoA
//! cutover branch (since reverted), `try_set_existing` left a
//! `(key, Nil)` zombie node behind while `set_norm` already routed
//! Nil-writes through `soa_delete` — `next()` (which had switched its
//! filter to `meta_bits::is_live`) surfaced the zombie as a stray
//! `(key, nil)` pair in `pairs()` iteration.
//!
//! On the current chain-world develop tip, `next()`'s filters
//! (`tag != raw::NIL` for array, `!val.is_nil()` for nodes) mask the
//! immediate symptom. But the semantic divergence is a latent
//! ship-blocker for the next data-layout cutover, so these tests pin
//! the desired behaviour:
//!
//!   - SetField fast path (`Vm::newindex_step` → `try_set_existing`)
//!     writing `nil` to an existing key must remove it from
//!     `pairs()` iteration.
//!   - The behaviour must hold for array keys, hash-string keys, and
//!     hash-int keys promoted out of the array.
//!   - The full delete-then-iterate-empty loop from `6b5d16f`'s
//!     repro snippet must work end-to-end.
//!   - Re-inserting the same key after the Nil-write must restore
//!     the slot (covering the chain-world "soft tombstone is still
//!     `find_node`-reachable" contract).
//!
//! See `.dev/known-bugs/fixed/try-set-existing-tombstones-nil-val-live-slot.md`.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn eval(src: &str) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    }
}

fn eval_int(src: &str) -> i64 {
    let mut v = eval(src);
    assert_eq!(v.len(), 1, "expected 1 value from {src:?}");
    match v.pop().unwrap() {
        Value::Int(i) => i,
        other => panic!("expected Int from {src:?}, got {other:?}"),
    }
}

fn eval_bool(src: &str) -> bool {
    let mut v = eval(src);
    assert_eq!(v.len(), 1, "expected 1 value from {src:?}");
    match v.pop().unwrap() {
        Value::Bool(b) => b,
        other => panic!("expected Bool from {src:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Reproducer from 6b5d16f (post-revert, see commit msg) — array keys.
// SetField fast path writes Nil; `next(t)` must report empty.
// ---------------------------------------------------------------------

#[test]
fn array_keys_nil_write_via_setfield_clears_iteration() {
    // SetField fast path on integer keys 1, 3, 5 (all land in the
    // array part). `for k in pairs(t) do t[k] = nil end` exercises
    // `try_set_existing` from `newindex_step`.
    let src = r#"
        local t = {}
        t[1] = "a"; t[3] = "b"; t[5] = "c"
        for k in pairs(t) do t[k] = nil end
        return next(t) == nil
    "#;
    assert!(eval_bool(src), "next(t) should be nil after array Nil-writes");
}

// ---------------------------------------------------------------------
// Hash (string) keys land in the node table, exercising the
// `find_node` + soft-tombstone branch of `clear_existing_slot`.
// ---------------------------------------------------------------------

#[test]
fn hash_string_keys_nil_write_via_setfield_clears_iteration() {
    let src = r#"
        local t = {}
        t.a = 1; t.b = 2; t.c = 3
        t.b = nil
        local count = 0
        for k, v in pairs(t) do
            count = count + 1
            assert(v ~= nil, "pairs() must not surface (key, nil)")
        end
        return count
    "#;
    assert_eq!(eval_int(src), 2);
}

#[test]
fn hash_string_keys_full_clear_leaves_next_empty() {
    let src = r#"
        local t = {}
        t.a = 1; t.b = 2; t.c = 3
        for k in pairs(t) do t[k] = nil end
        return next(t) == nil
    "#;
    assert!(eval_bool(src), "next(t) should be nil after hash Nil-writes");
}

// ---------------------------------------------------------------------
// Hash-int keys that miss the array part (sparse ints) — these
// also exercise the node-table branch.
// ---------------------------------------------------------------------

#[test]
fn sparse_int_keys_nil_write_clears_iteration() {
    let src = r#"
        local t = {}
        t[1000] = "x"; t[2000] = "y"
        t[1000] = nil
        local count = 0
        local seen_2000 = false
        for k, v in pairs(t) do
            count = count + 1
            assert(v ~= nil, "pairs() must not surface (key, nil)")
            if k == 2000 then seen_2000 = true end
        end
        assert(seen_2000)
        return count
    "#;
    assert_eq!(eval_int(src), 1);
}

// ---------------------------------------------------------------------
// Chain-world "soft tombstone is still find_node-reachable" contract.
// After `t.k = nil` via the fast path, `t.k = v` must put the slot
// back without losing other entries.
// ---------------------------------------------------------------------

#[test]
fn nil_write_then_reinsert_round_trip_hash() {
    let src = r#"
        local t = {}
        t.a = 1; t.b = 2; t.c = 3
        t.b = nil
        t.b = 42
        return t.a + t.b + t.c
    "#;
    assert_eq!(eval_int(src), 1 + 42 + 3);
}

#[test]
fn nil_write_then_reinsert_round_trip_array() {
    let src = r#"
        local t = {}
        t[1] = 10; t[2] = 20; t[3] = 30
        t[2] = nil
        t[2] = 200
        return t[1] + t[2] + t[3]
    "#;
    assert_eq!(eval_int(src), 10 + 200 + 30);
}

// ---------------------------------------------------------------------
// __newindex semantics on a slot just cleared by a SetField Nil-write.
// After `t.k = nil`, the slot is observably absent — a subsequent
// `t.k = v` with a `__newindex` metatable must fire `__newindex`.
// This pins the (slot_nil ⇔ fire_newindex) invariant from the A3
// audit across the tombstoned state.
// ---------------------------------------------------------------------

#[test]
fn newindex_fires_after_nil_write_clears_slot() {
    let src = r#"
        local fallback = setmetatable({}, {})
        local t = setmetatable({}, {
            __newindex = function(tab, k, v) fallback[k] = v end,
        })
        rawset(t, "a", 1)
        t.a = nil               -- clears slot via try_set_existing
        t.a = 99                -- now slot_nil → __newindex fires
        return fallback.a
    "#;
    assert_eq!(eval_int(src), 99);
}

// ---------------------------------------------------------------------
// Mixed delete pattern stress: alternating array + hash keys cleared
// via the fast path. Verifies the two halves of `clear_existing_slot`
// route correctly off the same call site.
// ---------------------------------------------------------------------

#[test]
fn mixed_array_hash_nil_writes_leave_only_survivors() {
    let src = r#"
        local t = {}
        t[1] = "one"; t[2] = "two"; t[3] = "three"
        t.x = "X"; t.y = "Y"; t.z = "Z"
        t[2] = nil
        t.y = nil
        local count = 0
        local sum_idx = 0
        local has_x, has_z = false, false
        for k, v in pairs(t) do
            count = count + 1
            assert(v ~= nil, "pairs() must not surface (key, nil)")
            if type(k) == "number" then sum_idx = sum_idx + k end
            if k == "x" then has_x = true end
            if k == "z" then has_z = true end
        end
        assert(count == 4)
        assert(sum_idx == 1 + 3)
        assert(has_x and has_z)
        return count
    "#;
    assert_eq!(eval_int(src), 4);
}
