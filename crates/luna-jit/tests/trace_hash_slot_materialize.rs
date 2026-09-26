//! Hash slot materialise.
//! `luna_jit_materialize_sunk_table` takes 7 i64 args (cap, arr_raws,
//! arr_kinds, n_hash, hash_keys, hash_raws, hash_kinds).
//! emit_materialize_live_sunk stack-allocs 3 parallel hash buffers per
//! site, fills them from virt_vars[cap..cap+n_hash] + virt_kinds +
//! head_proto.consts (for key ptrs), then calls the helper.
//!
//! Hash sites survive cmp side-exits via `table.set(Value::Str(key),
//! …)` at materialise time.

use luna_jit::version::LuaVersion;

/// dict + cmp body: hash site stays Sinkable, cmp side-exit
/// reconstructs the table via the extended materialise helper.
#[test]
fn dict_with_cmp_now_sunk_emits() {
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
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "dict+cmp must take sunk path (not demoted \
         by has_any_cmp); got sunk_alloc={}, compiled={}",
        vm.trace_sunk_alloc_count(),
        vm.trace_compiled_count(),
    );
}

/// Sanity: array sunk regression. The helper's signature
/// must still handle pure-array sites (n_hash=0, null hash ptrs).
#[test]
fn array_sunk_still_works_with_hash_materialize() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1, 2, 3}
                 if i > 500 then s = s + t[1] end
             end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(500)));
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "array-only sunk path must still emit; got sunk_alloc={}",
        vm.trace_sunk_alloc_count()
    );
    assert!(
        vm.trace_materialize_emit_count() >= 1,
        "cmp side-exit must emit materialise call; got mat={}",
        vm.trace_materialize_emit_count()
    );
}

/// Mixed: dict with both string key + array entry. Single site
/// has array_cap > 0 AND hash_keys.len() > 0.
#[test]
fn dict_plus_array_sunk_works() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 500 do
                 local t = {99}
                 t.name = i
                 s = s + t[1] + t.name
             end
             return s",
        )
        .unwrap();
    // (99 + i) for i=1..500 = 99*500 + 500*501/2 = 49500 + 125250 = 174750.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(174750)),
        "expected Int(174750), got {:?}",
        r[0]
    );
}
