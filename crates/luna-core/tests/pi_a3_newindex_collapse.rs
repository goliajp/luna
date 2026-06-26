//! v2.0 PI Phase 2 attack A3 — `Table::try_set_existing` single-walk
//! collapse regression tests.
//!
//! The collapse fuses the prior `tb.get(key).is_nil()` gate and the
//! `raw_set` walk into one chain traversal when the key is already
//! present with a non-nil value. The `__newindex` chain semantics must
//! remain bit-identical to the PUC spec:
//!
//!   - present-with-non-nil → write in place, NEVER fire `__newindex`
//!   - absent / present-with-nil → fire `__newindex` (chain to metatable's
//!     `__newindex` table, then to its function, raising at chain-too-long)
//!   - NilIndex / NanIndex keys still surface as runtime errors via the
//!     `raw_set` fallback
//!
//! See `.dev/rfcs/v2.0-pi-phase2-a3-audit.md` §4 for the case-by-case
//! semantics analysis.

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

fn eval_err(src: &str) -> String {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => panic!("expected error from {src:?}, got values {v:?}"),
        Err(e) => vm.error_text(&e).to_string(),
    }
}

fn eval_int_pair(src: &str) -> (i64, i64) {
    let mut v = eval(src);
    assert_eq!(v.len(), 2, "expected 2 values from {src:?}");
    let b = v.pop().unwrap();
    let a = v.pop().unwrap();
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => (a, b),
        other => panic!("expected (Int, Int) from {src:?}, got {other:?}"),
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

// ---------------------------------------------------------------------
// Happy path: present-key writes should NOT fire __newindex
// ---------------------------------------------------------------------

#[test]
fn present_key_write_skips_newindex_no_meta() {
    // Bare table, no metatable: present-key write is just an in-place
    // update via try_set_existing. Verifies the single-walk path doesn't
    // detour through any chain.
    let (a, b) = eval_int_pair(
        r#"
        local t = { x = 1, y = 2 }
        t.x = 10
        t.y = 20
        return t.x, t.y
    "#,
    );
    assert_eq!((a, b), (10, 20));
}

#[test]
fn present_key_write_skips_newindex_metatable_present() {
    // Metatable __newindex set but key already present: writes go
    // straight to the table, __newindex is NEVER called. Counter
    // tracks whether the metamethod fired (must remain 0).
    let v = eval_int(
        r#"
        local fires = 0
        local t = { x = 1 }
        setmetatable(t, {
            __newindex = function(tbl, key, val)
                fires = fires + 1
                rawset(tbl, key, val)
            end,
        })
        t.x = 99      -- present → in-place, __newindex must NOT fire
        return fires
    "#,
    );
    assert_eq!(v, 0, "__newindex fired despite present-with-non-nil key");
}

#[test]
fn absent_key_fires_newindex_function() {
    // Key absent → __newindex function metamethod runs. We capture
    // (key, val) so we know the dispatcher routed correctly.
    let v = eval_int(
        r#"
        local seen_key = nil
        local seen_val = nil
        local t = {}
        setmetatable(t, {
            __newindex = function(_, k, v)
                seen_key = k
                seen_val = v
            end,
        })
        t.fresh = 42
        if seen_key == "fresh" and seen_val == 42 then
            return 1
        else
            return 0
        end
    "#,
    );
    assert_eq!(v, 1);
}

#[test]
fn present_with_nil_fires_newindex() {
    // Tricky case: key was assigned nil, so observable value is nil →
    // __newindex must fire on the next write. Validates that
    // try_set_existing returns false on present-but-nil slots.
    let v = eval_int(
        r#"
        local fires = 0
        local t = { x = 1 }
        t.x = nil  -- clears the slot
        setmetatable(t, { __newindex = function(_, _, _) fires = fires + 1 end })
        t.x = 99   -- absent observable → __newindex must fire
        return fires
    "#,
    );
    assert_eq!(v, 1);
}

// ---------------------------------------------------------------------
// __newindex chain ordering: metatable redirect to another table
// ---------------------------------------------------------------------

#[test]
fn newindex_redirect_to_table_then_present_short_circuits() {
    // PUC chain: A absent → A.metatable.__newindex (= B table) → B
    // gets the write. Subsequent writes to a key already in B should
    // hit B's present path on the SECOND loop iteration of
    // newindex_step (cur = B), exercising the single-walk fast path
    // mid-chain.
    let v = eval_int(
        r#"
        local fwd = {}
        local t = setmetatable({}, { __newindex = fwd })
        t.x = 7           -- absent → forwarded to fwd; fwd.x = 7
        t.x = 99          -- present in fwd via chain? in PUC: t.x is
                          --   absent on t (rawset only touched fwd), so
                          --   chain reruns; reaches fwd, fwd.x present,
                          --   single-walk in-place update.
        -- Cross-check: t direct is still empty
        local raw_t = rawget(t, "x")
        local raw_fwd = fwd.x
        if raw_t == nil and raw_fwd == 99 then
            return 1
        else
            return 0
        end
    "#,
    );
    assert_eq!(v, 1);
}

#[test]
fn newindex_chain_function_at_tail() {
    // Two-table chain ending in a function. Verifies that after one
    // table redirect, the chain still surfaces the function metamethod
    // (i.e., the single-walk collapse hasn't broken the chain advance).
    let v = eval_int(
        r#"
        local fires = 0
        local middle = setmetatable({}, {
            __newindex = function(_, _, _) fires = fires + 1 end,
        })
        local t = setmetatable({}, { __newindex = middle })
        t.new_key = 5
        return fires
    "#,
    );
    assert_eq!(v, 1);
}

#[test]
fn newindex_chain_too_long_raises() {
    // The chain depth bound is preserved — a self-cycle should hit
    // MAX_TAG_LOOP and raise the 'chain too long' error.
    let err = eval_err(
        r#"
        local loop = {}
        setmetatable(loop, { __newindex = loop })
        loop.x = 1
    "#,
    );
    assert!(
        err.contains("__newindex") && err.contains("chain too long"),
        "expected chain-too-long error, got {err:?}"
    );
}

// ---------------------------------------------------------------------
// Edge cases: nil / NaN / float-with-integer-value keys
// ---------------------------------------------------------------------

#[test]
fn nil_key_raises_table_index_error() {
    // Nil key must surface as a runtime error. try_set_existing
    // returns false on Nil, falling through to raw_set which raises.
    let err = eval_err(
        r#"
        local t = {}
        t[nil] = 1
    "#,
    );
    assert!(
        err.contains("table index is nil"),
        "expected nil-index error, got {err:?}"
    );
}

#[test]
fn nan_key_raises_table_index_error() {
    let err = eval_err(
        r#"
        local t = {}
        t[0/0] = 1
    "#,
    );
    assert!(
        err.contains("table index is NaN"),
        "expected NaN-index error, got {err:?}"
    );
}

#[test]
fn float_with_int_value_key_canonicalised() {
    // Float key with exact-int value normalises to Int — must hit the
    // same slot whether the key was written as 3 or 3.0.
    let v = eval_int(
        r#"
        local t = {}
        t[3.0] = 42
        return t[3]
    "#,
    );
    assert_eq!(v, 42);
}

// ---------------------------------------------------------------------
// Array-part slot present-but-nil: same fast path as hash side
// ---------------------------------------------------------------------

#[test]
fn array_present_with_nil_fires_newindex() {
    // Build a table with array slot 1 = something, then clear via nil
    // (slot is still in array range, but tag = nil). Set a __newindex
    // and verify next write fires the metamethod (slot was observable
    // as nil → must NOT silently update inline).
    let v = eval_int(
        r#"
        local fires = 0
        local t = { "first" }   -- t[1] = "first" lives in the array part
        t[1] = nil              -- array slot now nil-tagged
        setmetatable(t, { __newindex = function(_, _, _) fires = fires + 1 end })
        t[1] = "fresh"
        return fires
    "#,
    );
    assert_eq!(v, 1);
}

#[test]
fn array_present_with_value_skips_newindex() {
    // Mirror of the hash-side test: array slot non-nil → write in
    // place, __newindex must NOT fire.
    let v = eval_int(
        r#"
        local fires = 0
        local t = { 10, 20, 30 }
        setmetatable(t, { __newindex = function(_, _, _) fires = fires + 1 end })
        t[2] = 99
        return fires
    "#,
    );
    assert_eq!(v, 0);
}

// ---------------------------------------------------------------------
// Hot-path shape: token_bucket-style repeated SetField ops on the same
// keys. End-to-end correctness check (the PI Phase 1 cell workload).
// ---------------------------------------------------------------------

#[test]
fn token_bucket_repeated_set_field_correctness() {
    // Same shape as the criterion bench: 1000 iterations of SetField
    // on present keys. The single-walk collapse must produce
    // bit-identical results.
    let (tokens, refilled) = eval_int_pair(
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
        return bucket.tokens, refilled
    "#,
    );
    // Loop body: every iteration refills tokens to 1000 then
    // decrements by 1. After 1000 iters tokens = 999, refilled = 1000.
    assert_eq!(tokens, 999);
    assert_eq!(refilled, 1000);
}
