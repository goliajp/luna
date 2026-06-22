//! P12-S12-B v4 — TForCall batched helper + cranelift `ld` replaces
//! the per-iter `luna_jit_stack_load` and `luna_jit_stack_tag` calls.
//!
//! v3 (`1df1806`) had 4 helper calls per iter (op_tforcall +
//! stack_tag + 2×stack_load). v4 collapses 3 of them by passing 3
//! out-pointers to `luna_jit_op_tforcall` and emitting cranelift
//! `stack_load` IR from the buffer for the reload, plus stashing the
//! returned tag in a Variable that TForLoop tail reads via use_var.
//!
//! Tests verify correctness across patterns that exercise:
//! (1) the batched out-ptr writeback (ctrl/key/val all reachable),
//! (2) the tag Variable across block boundary (TForCall continue
//!     blk → TForLoop tail's nil-check),
//! (3) the trace head-resident in a separate proto so the trace
//!     hot-counter actually triggers on TForLoop (not on an outer
//!     numeric ForLoop sharing the chunk's proto).

use luna::version::LuaVersion;
use luna::vm::Vm;

/// 1M-element ipairs trace inside a function — gives the inner
/// generic-for proto its own trace_hot_count.
#[test]
fn ipairs_function_wrapped_compiles_and_dispatches() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for i = 1, 1000 do t[i] = i end
             local function sum_ipairs(tt)
                 local s = 0
                 for _, v in ipairs(tt) do
                     s = s + v
                 end
                 return s
             end
             return sum_ipairs(t)",
        )
        .unwrap();
    // sum 1..1000 = 500500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "expected at least one trace to compile; compiled_count={}",
        vm.trace_compiled_count(),
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "expected the inner-ipairs trace to dispatch; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Both keys + values must reach the body correctly — exercise the
/// `key_out` (R[A+4]) and `val_out` (R[A+5]) buffer writes
/// independently by reading both `i` and `v`.
#[test]
fn ipairs_function_reads_both_key_and_value() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for j = 1, 1000 do t[j] = j * 10 end
             local function sum_pairs(tt)
                 local s = 0
                 for i, v in ipairs(tt) do
                     s = s + i + v
                 end
                 return s
             end
             return sum_pairs(t)",
        )
        .unwrap();
    // sum_i = 500500; sum_v = 5005000; total = 5505500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(5505500)),
        "expected Int(5505500), got {:?}",
        r[0]
    );
}

/// Multiple invocations of the same function — ensures the v4
/// `tforcall_tag_var` Variable behaves correctly across re-entries
/// (each dispatch must re-establish the tag from the helper's
/// return value, not carry a stale value across calls).
#[test]
fn ipairs_function_multiple_invocations_consistent() {
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
             local a, b, c = {1,2,3}, {10,20,30,40}, {100}
             return sum_t(a) + sum_t(b) + sum_t(c) + sum_t(b) + sum_t(a)",
        )
        .unwrap();
    // sum_t(a)=6, sum_t(b)=100, sum_t(c)=100; 6+100+100+100+6 = 312.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(312)),
        "expected Int(312), got {:?}",
        r[0]
    );
}
