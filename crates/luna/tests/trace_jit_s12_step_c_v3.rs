//! P12-S12-C v3 — ipairs `val_tag` runtime guard.
//!
//! v2 unlocked ipairs+Concat for uniform-Str arrays via implicit
//! specialisation on `entry_tag of R[A+5]`, but mixed-tag arrays
//! (e.g. `{'a', 1, 'c'}`) had a real correctness hole — the
//! v5 inline aget fast_blk would pack non-Str raw bits as a Str
//! pointer on the v2 spill path, producing garbage.
//!
//! v3 adds a runtime guard inside fast_blk: after loading
//! `val_tag` from the Table's atag byte, emit
//! `is_nil OR val_tag == expected_tag` and brif. Mismatch → deopt
//! to interp (which handles arbitrary tags correctly).
//!
//! The expected tag is snapshotted at recorder fire by reading
//! `vm.stack[base + a + 5].unpack().0` and stashing it in
//! `TraceRecord.tfor_val_tag`. The compile-time emit uses it as
//! an `iconst` immediate against the runtime `val_tag` load.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// v2 regression smoke: uniform-Str array still works post-v3
/// (guard always matches → no deopts on this shape).
#[test]
fn ipairs_uniform_str_array_no_regression() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {'a', 'b', 'c'}
             local function join(tt)
                 local s = ''
                 for _, v in ipairs(tt) do s = s .. v end
                 return s
             end
             local last = ''
             for i = 1, 30 do last = join(t) end
             return last",
        )
        .unwrap();
    match r[0] {
        luna::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"abc");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
    assert!(
        vm.trace_dispatched_count() >= 1,
        "uniform Str trace should still dispatch; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Mixed-tag array: snapshot picks the first iter's val tag, but
/// later iters deliver different tags → guard deopts → interp
/// runs the iter correctly. Tests that the result is correct
/// (Lua coerces Int → string in concat) and no panic / garbage.
#[test]
fn ipairs_mixed_tag_array_deopts_no_garbage() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {'a', 1, 'c'}
             local function join(tt)
                 local s = ''
                 for _, v in ipairs(tt) do s = s .. v end
                 return s
             end
             local last = ''
             for i = 1, 30 do last = join(t) end
             return last",
        )
        .unwrap();
    match r[0] {
        luna::runtime::Value::Str(ref s) => {
            // Lua concat coerces Int 1 → '1'.
            assert_eq!(s.as_bytes(), b"a1c");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}

/// Uniform-Int array: snapshot = INT, so guard expects INT.
/// `s = s + v` arithmetic body (no Concat) traces with the Int
/// path; just verifies the v3 guard's INT-snapshot variant
/// doesn't break the existing v5 ipairs Int trace.
#[test]
fn ipairs_uniform_int_array_arith_still_works() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for i = 1, 500 do t[i] = i end
             local function sum(tt)
                 local s = 0
                 for _, v in ipairs(tt) do s = s + v end
                 return s
             end
             return sum(t)",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(125250)),
        "expected Int(125250), got {:?}",
        r[0]
    );
}
