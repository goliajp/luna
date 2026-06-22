//! P12-S10-A — depth>0 site sunk emit. The `site.inline_depth != 0`
//! demote gate is lifted; emit_materialize_live_sunk now uses
//! `op_offsets[site.op_idx]` to address inline-frame sites' window.
//! `has_inline_cmp` gate stays (S10-B's domain).
//!
//! S10-A enables inline sites in traces with NO cmp at depth>0.
//! For binary_trees, `make`'s body has `if d == 0` at depth>0 →
//! `has_inline_cmp` still demotes. Patterns where the inline call
//! has NO cmp inside its body (e.g. a tail Closure) can sunk-emit
//! inline sites.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// Result correctness: depth>0 sites must round-trip semantics
/// correctly. binary_trees pattern is the canonical multi-depth
/// shape; even though `has_inline_cmp` demotes the inner make
/// site, the result must stay correct.
#[test]
fn binary_trees_pattern_correct_post_s10a() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function make(d)
                 if d == 0 then return {nil, nil} end
                 return {make(d-1), make(d-1)}
             end
             local function check(t)
                 if t[1] == nil then return 1 end
                 return 1 + check(t[1]) + check(t[2])
             end
             local sum = 0
             for i = 1, 10 do sum = sum + check(make(10)) end
             return sum",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(20470)),
        "expected Int(20470), got {:?}",
        r[0]
    );
}

/// fib regression: deep self-recursion (depth>0) with NewTable-less
/// body shouldn't be affected by S10-A. fib(28) trace must still
/// compile + dispatch as before.
#[test]
fn fib_28_still_dispatches_post_s10a() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n) if n<2 then return n end return f(n-1)+f(n-2) end
             return f(28)",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(317811)));
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib(28) must still dispatch; got {}",
        vm.trace_dispatched_count()
    );
}

/// S6 sunk_loadnil pattern regression: depth=0 site sunk emit
/// must still work. `local t = {nil, nil}; if t[1] == nil`
/// inside a hot for-loop body.
#[test]
fn sunk_loadnil_for_body_still_sunk_emits() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {nil, nil}
                 if t[1] == nil then s = s + 1 end
             end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(1000)));
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "depth=0 sunk emit must keep working post-S10-A; got \
         sunk_alloc_count={}",
        vm.trace_sunk_alloc_count(),
    );
}
