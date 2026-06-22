//! P12-S5-B — actual sunk-emit for `NewTable` / `SetList` / `GetI`
//! in non-cmp, non-looping traces. NewTable becomes a no-op; the
//! array part lives as Cranelift `Variable`s; GetI is a `use_var`
//! of the corresponding virt slot.
//!
//! These tests verify:
//! 1. Positive — a call-triggered trace through a function with a
//!    local literal table actually takes the sunk path
//!    (`trace_sunk_alloc_count >= 1`) AND produces the correct
//!    result (dispatch must work end-to-end).
//! 2. Negative — a function that returns the table itself stays
//!    on the heap path (pre-emit demotion drops it since
//!    `return_a == site.a`).
//! 3. Negative — a GetI with a key past the array_cap escapes the
//!    site (sweep marks it Escaped on OOB), so the heap path stays.
//!    The trace still compiles and produces a correct (Nil) value.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// Positive path: f() = `local t = {1,2,3}; return t[1]+t[2]+t[3]`.
/// Site at R[1] is Sinkable; trace's NewTable is a no-op, GetIs are
/// `use_var`s on virt slots. Result is 1*200 + 2*200 + 3*200 = 1200.
#[test]
fn sunk_callee_table_dispatches_with_correct_sum() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f()
                 local t = {1, 2, 3}
                 return t[1] + t[2] + t[3]
             end
             local s = 0
             for _ = 1, 200 do s = s + f() end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1200)),
        "expected Int(1200), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "f's local table must take the sunk-emit path; \
         got sunk_alloc_count={}, sinkable_seen={}, compiled={}",
        vm.trace_sunk_alloc_count(),
        vm.trace_sinkable_seen_count(),
        vm.trace_compiled_count()
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "sunk trace must dispatch (entry/length-gate passes); \
         got dispatched={}",
        vm.trace_dispatched_count()
    );
    assert_eq!(
        vm.trace_deopt_count(),
        0,
        "no helper that can park a deopt fires on the sunk path; \
         got deopt={}",
        vm.trace_deopt_count()
    );
}

/// Negative path: f() = `local t = {1,2}; return t`. The Return1's
/// R[A] IS the site's slot — pre-emit demotion drops the site to
/// Escaped because v1 doesn't materialise on return. The trace
/// still compiles (heap path) and returns the table.
#[test]
fn returning_the_table_skips_sunk_path() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f()
                 local t = {10, 20}
                 return t
             end
             local last = nil
             for _ = 1, 200 do last = f() end
             return last[1] + last[2]",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(30)),
        "expected Int(30), got {:?}",
        r[0]
    );
    assert_eq!(
        vm.trace_sunk_alloc_count(),
        0,
        "f returns the table itself; pre-emit demotion blocks sunk \
         emit until S5-C adds the materialise helper. \
         got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
}

/// Negative path: f() reads `t[3]` from a 2-element literal table.
/// The escape sweep's GetI rule marks the site Escaped on OOB (the
/// sunk emit can't represent a `t[k]` read where `k > cap`). The
/// heap path stays; `t[3]` returns Nil per Lua semantics.
#[test]
fn get_i_oob_falls_back_to_heap_path() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f()
                 local t = {1, 2}
                 return t[3]
             end
             local last = nil
             for _ = 1, 200 do last = f() end
             return last",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Nil),
        "expected Nil (t[3] of 2-elem table), got {:?}",
        r[0]
    );
    assert_eq!(
        vm.trace_sunk_alloc_count(),
        0,
        "OOB GetI escapes the site; heap path must remain. \
         got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
}
