//! Embedding controls: instruction budget, memory cap, minimal VMs and native panics.

use super::*;
use luna_core::vm::error::LuaErrorKind;

#[test]
fn embedding_instr_budget_interrupts_infinite_loop() {
    // A small budget catches a runaway loop. pcall catches the raised
    // "instruction budget exceeded", but the budget stays exhausted, so
    // the `return` after the pcall raises it again: the embedder gets
    // control back, the script does not.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_instr_budget(Some(5_000));
    let err = vm
        .eval(
            "local ok, err = pcall(function () \
               while true do end \
             end) \
             return ok, err",
        )
        .expect_err("the budget error reaches the host");
    let msg = vm.error_text(&err);
    assert!(msg.contains("instruction budget exceeded"), "{msg}");
    assert_eq!(vm.error_kind(), LuaErrorKind::InstrBudget);
    // exhausted until the embedder arms a new budget
    assert_eq!(vm.instr_budget_remaining(), Some(0));
}

#[test]
fn embedding_instr_budget_unset_runs_normally() {
    // No budget set — the existing tests are proof, but pin it here
    // so a future change to the default doesn't silently break embedders
    // that never touch `set_instr_budget`.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval("local s = 0 for i=1,1000 do s = s + i end return s")
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(500_500)), "got {:?}", v[0]);
}

#[test]
fn embedding_new_minimal_has_no_globals() {
    // Sandbox: `new_minimal` leaves the globals table empty so the
    // embedder can choose exactly which libraries to expose. Probing for
    // `print` should raise "attempt to call a nil value".
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let result = vm.eval("print('hi')");
    match result {
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("attempt to call") || msg.contains("nil"),
                "expected nil-call error, got: {msg}"
            );
        }
        Ok(_) => panic!("print should not exist on a new_minimal vm"),
    }
}

#[test]
fn embedding_selective_open_base_enables_print() {
    // After `new_minimal`, `open_base` is enough to make `print`
    // and friends resolve. The host can keep math/io/debug/os out.
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.open_base();
    // `tostring` is part of the base library — exercising it confirms the
    // open ran. We can't directly observe `print` without intercepting
    // stdout, but `type(print)` works.
    let v = vm
        .eval("return type(print), type(tostring), tostring(42)")
        .unwrap();
    assert_eq!(v.len(), 3);
    assert!(matches!(v[0], Value::Str(_)));
    if let Value::Str(s) = v[0] {
        assert_eq!(s.as_bytes(), b"function");
    }
    if let Value::Str(s) = v[2] {
        assert_eq!(s.as_bytes(), b"42");
    }
    // math is *not* opened, so `math` is nil.
    let v = vm.eval("return math").unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Nil));
}

#[test]
fn embedding_memory_cap_catches_runaway_alloc() {
    // Soft cap: build a tight loop that allocates tables; the run loop
    // detects bytes > cap between dispatch turns, runs a collect, and
    // (still over) raises a catchable error. The cap path runs a full
    // collect before deciding to fire, so short-lived intermediates do
    // not trip — the inner loop must hold enough live state to push past
    // the post-collect threshold.
    //
    // luna-core's `Vm::new` defaults to `NullJitBackend`; the interp
    // loop ticks slow enough that GC has breathing room between alloc
    // bursts, which would mask the cap trip. So the inner loop holds
    // **all** allocated tables in a live array — no intermediate gets
    // reclaimed, and the cap fires on net live bytes rather than on
    // burst-vs-GC timing.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let baseline = vm.memory_used();
    vm.set_memory_cap(Some(baseline + 64 * 1024)); // small headroom
    let err = vm
        .eval(
            "local outer = {} \
             local ok, err = pcall(function () \
               for i = 1, 1000000 do outer[i] = string.rep('x', 100) end \
               return outer \
             end) \
             return ok, err",
        )
        .expect_err("the cap error reaches the host after the pcall caught it");
    let msg = vm.error_text(&err);
    assert!(msg.contains("memory cap exceeded"), "{msg}");
    assert_eq!(vm.error_kind(), LuaErrorKind::MemoryCap);
}

#[test]
fn embedding_kevy_shape_short_script_per_request() {
    // Script-host shape: a Redis-style server gets many short scripts from
    // clients. Each call re-arms the budget, evaluates, harvests the
    // result, and the same Vm continues for the next request — possibly
    // after the previous one tripped its budget. Pin the round-trip.
    let mut vm = Vm::new(LuaVersion::Lua55);

    // (1) Normal short script with a generous budget.
    vm.set_instr_budget(Some(10_000));
    let v = vm
        .eval("local s = 0 for i=1,100 do s = s + i end return s")
        .unwrap();
    assert!(matches!(v[0], Value::Int(5050)));
    // Budget consumed but not tripped; some remaining.
    assert!(vm.instr_budget_remaining().unwrap_or(0) > 0);

    // (2) Trip the budget on the next request. The error propagates because
    // the embedder didn't wrap in pcall; the host catches it and continues.
    vm.set_instr_budget(Some(500));
    let err = vm.eval("while true do end").expect_err("budget must trip");
    let msg = vm.error_text(&err);
    assert!(msg.contains("instruction budget"), "got: {msg}");
    // After the trip the budget stays exhausted: a request the host did
    // not re-arm for raises at its first instruction.
    assert_eq!(vm.instr_budget_remaining(), Some(0));
    let err = vm
        .eval("return 1")
        .expect_err("no new budget, no instruction");
    assert!(vm.error_text(&err).contains("instruction budget"));

    // (3) Re-arm and run again — Vm state survived the budget trip cleanly.
    vm.set_instr_budget(Some(10_000));
    let v = vm
        .eval("local t = {1,2,3,4,5}; local s = 0 for _, x in ipairs(t) do s = s + x end return s")
        .unwrap();
    assert!(matches!(v[0], Value::Int(15)));

    // (4) Globals persist across requests — the host can pin shared state.
    vm.set_global("counter", Value::Int(0)).unwrap();
    for expected in 1..=5 {
        vm.set_instr_budget(Some(10_000));
        let v = vm.eval("counter = counter + 1; return counter").unwrap();
        assert!(
            matches!(v[0], Value::Int(n) if n == expected),
            "iter {expected}: got {:?}",
            v[0],
        );
    }
}

#[test]
fn embedding_memory_cap_unset_runs_normally() {
    // No cap = no enforcement. Allocates ~4MB of integer-keyed strings and
    // returns count to prove the loop ran to completion.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval(
            "local t = {} \
             for i = 1, 10000 do t[i] = tostring(i) end \
             return #t",
        )
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(10000)));
}

fn panic_string_native(
    _vm: &mut Vm,
    _fs: u32,
    _nargs: u32,
) -> Result<u32, luna_core::vm::LuaError> {
    panic!("boom from a native");
}

fn panic_static_str_native(
    _vm: &mut Vm,
    _fs: u32,
    _nargs: u32,
) -> Result<u32, luna_core::vm::LuaError> {
    panic!("static boom");
}

#[test]
fn embedding_native_panic_caught_as_lua_error() {
    // A Rust panic inside a registered native must not unwind through
    // the dispatch loop. The catch_unwind in begin_call's native arm folds
    // it into a "native panic: <msg>" Lua error that pcall can catch.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f1 = vm.native(panic_string_native);
    vm.set_global("p1", f1).unwrap();
    let f2 = vm.native(panic_static_str_native);
    vm.set_global("p2", f2).unwrap();
    let v = vm.eval("return pcall(p1)").expect("pcall returns normally");
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        let msg = String::from_utf8_lossy(s.as_bytes());
        assert!(
            msg.contains("native panic") && msg.contains("boom from a native"),
            "string-payload panic should surface: {msg}"
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
    let v = vm.eval("return pcall(p2)").expect("pcall returns normally");
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        let msg = String::from_utf8_lossy(s.as_bytes());
        assert!(
            msg.contains("native panic") && msg.contains("static boom"),
            "static-str-payload panic should surface: {msg}"
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
}
