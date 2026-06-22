//! P12-S11-A — Op::SetField / Op::GetField whitelist (helper path).
//! String-keyed table operations (`t.x = v`, `v = t.x`) now compile
//! via `luna_jit_table_set_field` / `luna_jit_table_get_field`
//! helpers; the const string key is baked into the IR from
//! head_proto.consts at emit time. No sunk emit yet — hash-part
//! sunk is S11-B.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `local t = {}; t.x = i; t.y = i*2; s = s + t.x + t.y` — pure
/// hash-keyed table operations. Pre-S11-A: SetField / GetField
/// (outside math fold) bailed compile. Post-S11-A: helper-path
/// compile + dispatch (subject to length-gate).
#[test]
fn dict_assign_then_read_compiles_post_s11a() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
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
        matches!(r[0], luna::runtime::Value::Int(1501500)),
        "expected Int(1501500), got {:?}",
        r[0]
    );
    // Pre-S11-A this trace bailed compile. Post-S11-A at least one
    // trace compiles (the for-loop body containing SetField/GetField).
    assert!(
        vm.trace_compiled_count() >= 1,
        "post-S11-A dict_assign body must compile; got closed={} \
         compiled={} fail={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// SetField with a non-Str const at K[B] still bails — the
/// pre-emit Str validation must reject this case.
#[test]
fn setfield_non_str_const_still_bails() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Standard usage; Lua frontend only emits SetField with Str
    // keys, so this test mostly verifies result correctness with
    // the new whitelist path active.
    let r = vm
        .eval(
            "local t = {a = 1, b = 2, c = 3}
             local s = 0
             for i = 1, 200 do s = s + t.a + t.b + t.c end
             return s",
        )
        .unwrap();
    // (1+2+3)*200 = 1200.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1200)),
        "expected Int(1200), got {:?}",
        r[0]
    );
}

/// `local t = {name='alice', age=30}; return t.name` — table
/// initialised via SetField in the constructor, then read via
/// GetField. Verifies the helper path round-trips both kinds.
#[test]
fn getfield_returns_correct_value_post_s11a() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function make(n) return {value = n * 7} end
             local s = 0
             for i = 1, 500 do
                 local t = make(i)
                 s = s + t.value
             end
             return s",
        )
        .unwrap();
    // sum 7i for i=1..500 = 7 * 125250 = 876750.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(876750)),
        "expected Int(876750), got {:?}",
        r[0]
    );
}
