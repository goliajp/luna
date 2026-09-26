//! P12-S10-B — inline-cmp + materialise combine. The
//! `has_inline_cmp` gate was dropped: inline cmp side-exits
//! (`per_exit_inline` arm) now call `emit_materialize_live_sunk`
//! to reconstruct live sunk sites BEFORE the frame-materialize
//! helper pushes the inline frames.
//!
//! S10-B's measurable unlock requires a recorder that cleanly
//! closes a recursive body with inline-depth NewTable +
//! inline-depth cmp (binary_trees `make`'s shape). The recorder
//! currently exits the recursive trace via bail / depth cap /
//! length cap, leaving only leaf shapes closed. Per-trace tests
//! here verify correctness preservation + leaf-trace regression.

use luna_jit::version::LuaVersion;

/// binary_trees full program — result must be correct. Pre-S10-B
/// the `has_inline_cmp` gate demoted all sites when ANY cmp@d>0
/// appeared in the trace body. Post-S10-B sites survive + the
/// inline cmp side-exit reconstructs them. Even if the recursive
/// trace itself never dispatches (recorder doesn't close it
/// cleanly), the closed leaf traces + interp must produce the
/// right tree sum.
#[test]
fn binary_trees_correct_post_s10b() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
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
        matches!(r[0], luna_jit::runtime::Value::Int(20470)),
        "expected Int(20470), got {:?}",
        r[0]
    );
}

/// fib(28) regression: deep recursion with cmp at every depth.
/// Pre-S10-B fib had no NewTable so `has_inline_cmp` didn't gate
/// anything. Post-S10-B should still dispatch identically.
#[test]
fn fib_28_unchanged_post_s10b() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n) if n<2 then return n end return f(n-1)+f(n-2) end
             return f(28)",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(317811)));
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib(28) must still dispatch; got {}",
        vm.trace_dispatched_count()
    );
}

/// S5/S6/S8 sunk depth=0 patterns regression — make sure the
/// emit_materialize refactor's `&mut snapshot` API didn't break
/// depth=0 cmp materialise.
#[test]
fn sunk_loadnil_for_body_still_sunk_emits_post_s10b() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
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
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(1000)));
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "depth=0 sunk emit must keep working; got sunk_alloc={}",
        vm.trace_sunk_alloc_count()
    );
    assert!(
        vm.trace_materialize_emit_count() >= 1,
        "depth=0 cmp side-exit must materialise the live sunk \
         table; got mat_emit={}",
        vm.trace_materialize_emit_count()
    );
}
