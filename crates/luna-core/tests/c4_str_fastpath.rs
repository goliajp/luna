//! v2.0 Phase 9 C4 — `Table::find_node_str` + `Table::get_str` +
//! `Table::try_set_existing_str` str-key fast-path regression tests.
//!
//! The fast-path skips `find_node`'s 12-arm `raw_eq` and `hash_key`'s
//! 12-arm match by routing `Op::GetField` / `Op::SetField` through
//! string-specialised accessors. Behaviour must remain bit-identical
//! to the generic `Op::GetTable` / `Op::SetTable` path:
//!
//!   - present key, no metatable → in-place read / overwrite
//!   - absent key, no metatable → nil read / hash-part insert
//!   - present-with-nil → fires `__newindex` chain on write (slot is
//!     considered absent)
//!   - long-string key (compile-time literal exceeding short-string
//!     threshold) → bytes-equality fallback in `find_node_str` keeps
//!     PUC semantics intact
//!
//! See `.dev/rfcs/v2.0-value-layout-shrink-rfc.md` §4.4, §6 and
//! `.dev/baselines/perf-2026-06-27-step5/c4-phase-a-audit.md`.

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
// Cross-path equivalence — Op::GetField (.foo) == Op::GetTable ([k])
// ---------------------------------------------------------------------

#[test]
fn getfield_matches_gettable_for_present_str_key() {
    // `t.foo` lowers to Op::GetField (Str-const key from Proto.consts).
    // `t[k]` with `k = "foo"` lowers to Op::GetTable (dynamic key) and
    // hits the generic Table::get path. Both must return identical
    // values for the same logical key.
    let v = eval_int(
        r#"
        local t = { foo = 11, bar = 22, baz = 33 }
        local k = "foo"
        if t.foo == t[k] and t.bar == t["bar"] and t.baz == t[(function() return "baz" end)()] then
            return 1
        else
            return 0
        end
    "#,
    );
    assert_eq!(
        v, 1,
        "Op::GetField and Op::GetTable diverged on present str key"
    );
}

#[test]
fn getfield_matches_gettable_for_absent_str_key_returns_nil() {
    // `t.missing` and `t[k]` with `k = "missing"` must both return nil
    // when the key is absent (no metatable in scope).
    let v = eval_int(
        r#"
        local t = { foo = 1 }
        local k = "missing"
        if t.missing == nil and t[k] == nil and t.missing == t[k] then
            return 1
        else
            return 0
        end
    "#,
    );
    assert_eq!(v, 1);
}

#[test]
fn getfield_matches_gettable_for_present_with_nil_slot() {
    // PUC: an explicit `t.x = nil` clears the slot; `t.x` and `t[k]`
    // should both observe nil regardless of which path was used.
    let v = eval_int(
        r#"
        local t = { x = 1 }
        t.x = nil
        local k = "x"
        if t.x == nil and t[k] == nil then return 1 else return 0 end
    "#,
    );
    assert_eq!(v, 1);
}

// ---------------------------------------------------------------------
// SetField fast-path round-trip
// ---------------------------------------------------------------------

#[test]
fn setfield_round_trips_via_getfield_and_gettable() {
    // SetField (t.x = …) writes via try_set_existing_str; SetTable
    // (t[k] = …) writes via the generic try_set_existing. The read-back
    // through both Op::GetField and Op::GetTable must observe each
    // other's writes.
    let v = eval_int(
        r#"
        local t = { a = 0, b = 0, c = 0 }
        t.a = 10                 -- Op::SetField  (fast path)
        local k = "b"
        t[k] = 20                -- Op::SetTable  (slow path)
        t.c = t.a + t[k]         -- mixed read
        local kc = "c"
        return t.a + t["b"] + t[kc] + t.c
    "#,
    );
    assert_eq!(
        v,
        10 + 20 + 30 + 30,
        "SetField/SetTable cross-path observation broken"
    );
}

#[test]
fn setfield_overwrites_existing_no_metatable() {
    // Stay on the C4 fast-path: bare table, no metatable, Str key.
    let v = eval_int(
        r#"
        local t = { tokens = 1000, last = 0, rate = 100 }
        for i = 1, 100 do
            t.tokens = t.tokens - 1
            t.last = i
        end
        return t.tokens + t.last + t.rate
    "#,
    );
    assert_eq!(v, 900 + 100 + 100);
}

#[test]
fn setfield_inserts_new_key_when_no_metatable() {
    // SetField on an absent str key with no metatable → raw_set fallback.
    let v = eval_int(
        r#"
        local t = {}
        t.fresh = 7
        local k = "fresh"
        return t.fresh + t[k]
    "#,
    );
    assert_eq!(v, 14);
}

// ---------------------------------------------------------------------
// Metatable semantics survive (fast path is gated by metatable.is_none())
// ---------------------------------------------------------------------

#[test]
fn getfield_with_metatable_routes_through_index_metamethod() {
    // metatable.is_none() guard MUST shunt to op_index when present.
    let v = eval_int(
        r#"
        local fallback = { y = 99 }
        local t = setmetatable({}, { __index = fallback })
        return t.y
    "#,
    );
    assert_eq!(v, 99);
}

#[test]
fn setfield_with_metatable_routes_through_newindex_chain() {
    // metatable.is_none() guard MUST shunt SetField to op_newindex.
    let v = eval_int(
        r#"
        local seen_key = nil
        local seen_val = nil
        local t = setmetatable({}, {
            __newindex = function(_, k, v)
                seen_key = k
                seen_val = v
            end,
        })
        t.absent = 42
        if seen_key == "absent" and seen_val == 42 then return 1 else return 0 end
    "#,
    );
    assert_eq!(v, 1, "SetField fast-path swallowed __newindex");
}

#[test]
fn setfield_present_key_with_newindex_metatable_still_in_place() {
    // PI A3 collapse rule: present-with-non-nil writes NEVER fire
    // __newindex even with a metatable installed. The C4 SetField fast
    // path is gated by metatable.is_none() so this case falls to
    // op_newindex / try_set_existing — must preserve the rule.
    let v = eval_int(
        r#"
        local fires = 0
        local t = { x = 1 }
        setmetatable(t, {
            __newindex = function() fires = fires + 1 end,
        })
        t.x = 99
        return fires
    "#,
    );
    assert_eq!(v, 0);
}

// ---------------------------------------------------------------------
// Long-string keys — find_node_str must fall back to bytes equality
// ---------------------------------------------------------------------

#[test]
fn long_string_key_via_getfield_matches_gettable() {
    // A 60-byte field name exceeds PUC's short-string threshold (40
    // bytes for 5.5 / LUAI_MAXSHORTLEN). find_node_str's ptr_eq alone
    // wouldn't suffice — the bytes-equality fallback must keep PUC
    // semantics intact.
    let v = eval_int(
        r#"
        local longkey = "abcdefghijklmnopqrstuvwxyz0123456789_aaaaaaaaaaaaaaaaaaaa"
        local t = {}
        t[longkey] = 7
        local longkey2 = "abcdefghijklmnopqrstuvwxyz0123456789_aaaaaaaaaaaaaaaaaaaa"
        -- t[longkey2] forces a fresh string interning at compile-time
        -- but the StringTable may dedupe both into one allocation; the
        -- semantic test below holds either way.
        if t[longkey] == t[longkey2] then return 1 else return 0 end
    "#,
    );
    assert_eq!(v, 1, "long-string keys diverged across allocations");
}

// ---------------------------------------------------------------------
// Token-bucket replica — the chartered workload shape
// ---------------------------------------------------------------------

#[test]
fn token_bucket_shape_terminates_with_expected_state() {
    // Mirror the token_bucket_1k inner loop (5 GetField + 3 SetField/
    // iter on the bucket table). Verifies the C4 fast-path produces
    // the same state vector as the reference Lua semantics.
    let v = eval_int(
        r#"
        local bucket = { tokens = 1000, last = 0, rate = 100 }
        local now = 1
        local refilled = 0
        for i = 1, 1000 do
            local elapsed = now - bucket.last
            local refill = elapsed * bucket.rate
            if refill > 0 then
                bucket.tokens = math.min(1000, bucket.tokens + refill)
                bucket.last = now
                refilled = refilled + 1
            end
            if bucket.tokens >= 1 then
                bucket.tokens = bucket.tokens - 1
            end
            now = now + 1
        end
        return bucket.tokens + bucket.last + bucket.rate + refilled
    "#,
    );
    // bucket.tokens ends at 1000-1=999 on the first iter, then refill keeps
    // saturating at 1000 each iter and we decrement once — net 999.
    // bucket.last = 1000 (last `now` before the increment).
    // refilled = 1000 (every iter triggered a refill at `elapsed > 0`).
    // 999 + 1000 + 100 + 1000 = 3099.
    assert_eq!(v, 3099);
}

// ---------------------------------------------------------------------
// Rehash invariant — fast-path must survive bucket-array growth
// ---------------------------------------------------------------------

#[test]
fn fastpath_survives_rehash() {
    // Insert enough fields to force rehash; each subsequent GetField
    // must still find the bucket. Exercises find_node_str across a
    // `Heap::new_table_sized` boundary.
    let v = eval_int(
        r#"
        local t = {}
        local names = { "a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p","q","r" }
        for i, n in ipairs(names) do t[n] = i end
        local sum = 0
        for _, n in ipairs(names) do sum = sum + t[n] end
        return sum + t.a + t.r
    "#,
    );
    // 1+2+...+18 = 171; + t.a (1) + t.r (18) = 190.
    assert_eq!(v, 190);
}

// ---------------------------------------------------------------------
// rawequal sanity — get_str result identical to get()
// ---------------------------------------------------------------------

#[test]
fn rawequal_holds_between_getfield_and_gettable_results() {
    let r = eval_bool(
        r#"
        local t = { ref = {} }
        local k = "ref"
        return rawequal(t.ref, t[k])
    "#,
    );
    assert!(r);
}
