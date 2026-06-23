//! P12-S8-C — `Op::SetTable` sunk emit via key const-fold.
//! `const_fold_int_key` walks backward from a SetTable looking
//! for a LoadI (via a Move chain) that pinned R[B]'s value to a
//! literal in 1..=cap. If found, tag `SetTableSunkWrite` and
//! emit the sunk path (same shape as SetI sunk write).

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// `local k = 2; local t = {nil, nil}; t[k] = i` — Move R[?]=R[k]
/// → SetTable. const_fold walks: SetTable.B → Move source → LoadI 2.
/// Resolves key=2, in cap=2, sunk-emits.
#[test]
fn settable_with_literal_local_var_key_sunk() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local k = 2
                 local t = {nil, nil}
                 t[k] = i
                 s = s + t[2]
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
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "SetTable with const-foldable key must take the sunk path; \
         got sunk_alloc_count={}, sinkable_seen={}, compiled={}",
        vm.trace_sunk_alloc_count(),
        vm.trace_sinkable_seen_count(),
        vm.trace_compiled_count(),
    );
}

/// `for i=1,N do local t={nil,nil}; t[i] = v end` — R[B] (the key
/// reg) is the ForLoop visible loop var R[A+3], not from any LoadI
/// the trace records inside the body. Walking back from SetTable
/// won't find a LoadI for R[B] within the body; the const-fold
/// returns None → the target site escapes → helper path runs.
/// Result is still correct via the heap helper.
#[test]
fn settable_with_loop_var_key_escapes_to_helper() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 500 do
                 local t = {nil, nil}
                 -- inner for to write at runtime-varying key
                 for j = 1, 2 do t[j] = j * i end
                 s = s + t[1] + t[2]
             end
             return s",
        )
        .unwrap();
    // For each i: t[1]=i, t[2]=2i → sum 3i. Total = 3*(1+...+500) = 375750.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(375750)),
        "expected Int(375750), got {:?}",
        r[0]
    );
    // No strong sunk_alloc_count assertion — depends on trace
    // shape (inner-for or outer-for body). The contract: result
    // is correct, no panics.
}

/// `local k = 5; local t = {nil}; t[k] = v` — k=5 out of cap=1
/// range → const-fold returns None (key OOB) → escape. Helper
/// path handles the OOB write semantically (resize the table).
#[test]
fn settable_oob_const_key_escapes_correctly() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 200 do
                 local k = 5
                 local t = {nil}
                 t[k] = i
                 s = s + t[5]
             end
             return s",
        )
        .unwrap();
    // sum 1..200 = 20100.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(20100)),
        "expected Int(20100), got {:?}",
        r[0]
    );
}
