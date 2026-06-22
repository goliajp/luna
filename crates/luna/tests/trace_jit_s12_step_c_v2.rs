//! P12-S12-C v2 — `RegKind::Str` + `ExitTag::Str` + Move
//! propagation across the emit pass.
//!
//! v1 (`cf6cb28`) shipped `Op::Concat` whitelist + helper but
//! Str operands fell into the `Unset` kind and routed through
//! `luna_jit_stack_update_raw` (preserves vm.stack's existing
//! tag) — which broke for fresh frames where vm.stack[temp]'s
//! tag was Nil (from push_frame init). v2 adds the `Str` variant
//! to both `RegKind` and `ExitTag`, threading it through:
//! - `from_entry_tag(STR) → Some(Str)`
//! - `kinds_to_exit_tags(Str → ExitTag::Str)`
//! - dispatcher restore: `ExitTag::Str → raw::STR`
//! - every `tag_byte` match in emit (spill / Op::Closure in_stack
//!   upval / set_int / set_raw paths)
//! - `Op::Concat` emit's operand spill now picks `raw::STR` (not
//!   `update_raw`) for `RegKind::Str` slots
//!
//! Unlocks `s = s .. v` over ipairs (Str-valued arrays) and any
//! Concat where operands flow through Move from Str entry slots.
//!
//! Known limitation (v3 future): the trace doesn't add a runtime
//! `val_tag == STR` guard inside the ipairs inline aget fast path.
//! For a mixed-tag array (e.g. `{'a', 1, 'c'}`), val_tag varies
//! per iter and the spill writes `pack(STR, int_bits)` → garbage
//! pointer. Tests stick to uniform-Str arrays.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `s = s .. v` over an ipairs-iterated Str array — the canonical
/// case v1 couldn't compile. Verifies correctness + dispatch.
#[test]
fn ipairs_concat_str_array_correctness_and_dispatch() {
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
        "expected ipairs+concat trace to dispatch; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Longer array — exercises many trace iters before the Nil exit.
#[test]
fn ipairs_concat_longer_str_array() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {'x', 'y', 'z', 'a', 'b'}
             local function join(tt)
                 local s = ''
                 for _, v in ipairs(tt) do s = s .. v end
                 return s
             end
             local last = ''
             for i = 1, 20 do last = join(t) end
             return last",
        )
        .unwrap();
    match r[0] {
        luna::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"xyzab");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}

/// Concat where one operand comes from a Str local (entry-tagged
/// STR) and the other from arith (Int kind) — verifies that
/// mixed kinds emit correct tag_byte for each slot.
#[test]
fn concat_str_local_with_int_arith() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local prefix = 'val='
             local function build(n)
                 local last = ''
                 for i = 1, n do
                     last = prefix .. (i * 2)
                 end
                 return last
             end
             return build(200)",
        )
        .unwrap();
    // last iter: i=200, i*2=400. 'val=' .. 400 = 'val=400'.
    match r[0] {
        luna::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"val=400");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}
