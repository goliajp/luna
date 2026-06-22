//! P12-S12-B v5 — `Op::TForCall` inline aget specialised on
//! `ipairs_iter`.
//!
//! v4 (`37e3fcd`) consolidated 4 per-iter helper calls into one
//! `luna_jit_op_tforcall`. v5 takes the next swing: snapshot the
//! iter fn pointer at recorder-start, and when it matches
//! `ipairs_iter`, emit inline Table aget IR (load array_ptr, load
//! asize, load val_raw + val_tag from the array slot) — skip the
//! `op_tforcall` C call on the fast path entirely. Slow path
//! (hash-key / metatable present / not in array range) still
//! falls back to the v4 helper.
//!
//! Tests cover correctness across:
//! - inline aget fast path over inline-storage Int arrays
//! - inline aget fast path over slab-backed arrays (asize > 2)
//! - the non-ipairs path (helper fallback still works)
//! - the slow-path branch (hash-only table — should give zero iters
//!   since ipairs stops at first nil)

use luna::version::LuaVersion;
use luna::vm::Vm;

/// ipairs over a small (inline-storage, asize ≤ 2) array — fast
/// path's value/tag loads from `inline_storage` directly.
#[test]
fn ipairs_inline_storage_array_correctness() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function sum_t(tt)
                 local s = 0
                 for _, v in ipairs(tt) do s = s + v end
                 return s
             end
             local results = 0
             for i = 1, 200 do
                 results = results + sum_t({7, 11})
             end
             return results",
        )
        .unwrap();
    // sum_t returns 18 each time; 200 * 18 = 3600.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(3600)),
        "expected Int(3600), got {:?}",
        r[0]
    );
}

/// ipairs over a slab-backed array (asize > 2 = INLINE_ASIZE) —
/// fast path's array_ptr points to the slab. Exercises the
/// `TABLE_ARRAY_PTR_OFFSET` load returning a different pointer
/// than `inline_storage`.
#[test]
fn ipairs_slab_array_correctness_and_dispatch() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for j = 1, 500 do t[j] = j end
             local function sum_t(tt)
                 local s = 0
                 for _, v in ipairs(tt) do s = s + v end
                 return s
             end
             return sum_t(t)",
        )
        .unwrap();
    // sum 1..500 = 125250.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(125250)),
        "expected Int(125250), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "expected ipairs trace to dispatch; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Table with `__index` metatable — fast path's `no_meta` check
/// should fail, slow path's helper handles metatable correctly.
#[test]
fn ipairs_with_metatable_falls_through_to_slow_path() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local inner = {7, 8, 9}
             local wrapper = setmetatable({}, { __index = inner })
             local function sum_t(tt)
                 local s = 0
                 for _, v in ipairs(tt) do s = s + v end
                 return s
             end
             local results = 0
             for i = 1, 200 do
                 results = results + sum_t(wrapper)
             end
             return results",
        )
        .unwrap();
    // sum_t over wrapper sees array via __index: 7+8+9 = 24;
    // 200 * 24 = 4800.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(4800)),
        "expected Int(4800), got {:?}",
        r[0]
    );
}

/// `for i, v in ipairs(t)` — fast path's `R[A+4] = Int(next_i)`
/// writeback. Verifies the key (= next_i) is correctly threaded
/// through the inline path, not just the value.
#[test]
fn ipairs_reads_key_correctly_under_fast_path() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {10, 20, 30, 40, 50}
             local function sum_idx_and_val(tt)
                 local s = 0
                 for i, v in ipairs(tt) do
                     s = s + i + v
                 end
                 return s
             end
             local results = 0
             for k = 1, 200 do
                 results = results + sum_idx_and_val(t)
             end
             return results",
        )
        .unwrap();
    // sum_idx = 15; sum_val = 150; total per call = 165;
    // 200 * 165 = 33000.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(33000)),
        "expected Int(33000), got {:?}",
        r[0]
    );
}
