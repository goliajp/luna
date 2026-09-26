//! P12-S11-B-v1 — hash-part sunk emit. NewTable site admits any
//! shape (b/c don't constrain anymore); SetField on a bound site
//! allocates a per-key hash slot; GetField on a known key reads
//! from the same slot. virt_vars index space:
//!   [0 .. array_cap)              → array slots
//!   [array_cap .. array_cap+n_hash) → hash slots
//!
//! Materialise (deopt) helper only knows array slots, so hash
//! sites demote when the trace has ANY cmp (S11-B.2 will extend
//! materialise with hash slot support).

use luna_jit::version::LuaVersion;

/// dict_simple pattern: `local t = {}; t.x = i; t.y = i*2; s = s
/// + t.x + t.y`. No cmp in body → hash site stays Sinkable; SetField
/// + GetField sunk-emit through virt slots. The trace dispatches
/// without touching the heap.
#[test]
fn dict_assign_then_read_sunk_dispatches() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {}
                 t.x = i
                 t.y = i * 2
                 s = s + t.x + t.y
             end
             return s",
        )
        .unwrap();
    // sum (i + 2i) for i=1..1000 = 1501500.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(1501500)),
        "expected Int(1501500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "post-S11-B-v1 dict_assign body must take the sunk path; \
         got sunk_alloc_count={}, sinkable_seen={}, compiled={}",
        vm.trace_sunk_alloc_count(),
        vm.trace_sinkable_seen_count(),
        vm.trace_compiled_count(),
    );
}

/// dict with cmp in body — hash site demoted (materialise helper
/// can't reconstruct hash slots yet). Helper path runs; result
/// stays correct.
#[test]
fn dict_with_cmp_falls_back_to_helper_path() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {}
                 t.x = i
                 if t.x > 0 then s = s + t.x end
             end
             return s",
        )
        .unwrap();
    // sum 1..1000 = 500500.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
    // hash sunk is demoted by has_any_cmp gate; the trace may
    // still compile (helper path through SetField/GetField from
    // S11-A) but `sunk_alloc_count` won't include hash sites.
    let _ = vm.trace_sunk_alloc_count();
}

/// GetField for an unknown key (no prior SetField on this site)
/// escapes the site — without the escape, the trace would read
/// from an uninitialised virt slot. Result correctness check.
#[test]
fn getfield_unknown_key_escapes_correctly() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Read t.x BEFORE any write — should be nil (default).
    // The escape sweep marks the site Escaped on this GetField;
    // SetField in interp populates the heap table; final read
    // matches.
    let r = vm
        .eval(
            "local count = 0
             for i = 1, 500 do
                 local t = {}
                 if t.first == nil then count = count + 1 end
                 t.first = i
             end
             return count",
        )
        .unwrap();
    // Every iter's `t.first == nil` is true (fresh empty table).
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(500)),
        "expected Int(500), got {:?}",
        r[0]
    );
}
