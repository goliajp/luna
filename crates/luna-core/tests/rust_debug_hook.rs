//! B11 smoke tests: Rust-side debug hook.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::exec::{
    HOOK_MASK_CALL, HOOK_MASK_COUNT, HOOK_MASK_LINE, HOOK_MASK_RETURN, RustHookEvent,
};
use std::cell::RefCell;

thread_local! {
    static EVENTS: RefCell<Vec<RustHookEvent>> = const { RefCell::new(Vec::new()) };
}

fn record_hook(_vm: &mut Vm, event: RustHookEvent) {
    EVENTS.with(|e| e.borrow_mut().push(event));
}

fn fresh() -> Vm {
    EVENTS.with(|e| e.borrow_mut().clear());
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

#[test]
fn call_hook_fires_on_function_entry() {
    let mut vm = fresh();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_CALL, 0);
    let _ = vm
        .eval("local function f() return 1 end; f(); f(); f()")
        .unwrap();
    let calls = EVENTS.with(|e| {
        e.borrow()
            .iter()
            .filter(|ev| matches!(ev, RustHookEvent::Call))
            .count()
    });
    // Each f() call fires Call; we called 3 times.
    assert!(calls >= 3, "expected ≥3 Call events, got {calls}");
}

#[test]
fn return_hook_fires_on_function_return() {
    let mut vm = fresh();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_RETURN, 0);
    let _ = vm
        .eval("local function f() return 1 end; f(); f()")
        .unwrap();
    let rets = EVENTS.with(|e| {
        e.borrow()
            .iter()
            .filter(|ev| matches!(ev, RustHookEvent::Return | RustHookEvent::TailCall))
            .count()
    });
    assert!(rets >= 2, "expected ≥2 Return events, got {rets}");
}

#[test]
fn count_hook_fires_at_instruction_interval() {
    let mut vm = fresh();
    // Fire every 100 instructions.
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_COUNT, 100);
    let _ = vm
        .eval("local s = 0; for i = 1, 500 do s = s + i end")
        .unwrap();
    let counts = EVENTS.with(|e| {
        e.borrow()
            .iter()
            .filter(|ev| matches!(ev, RustHookEvent::Count))
            .count()
    });
    // 500-iter loop dispatches many opcodes; at granularity 100 we
    // expect multiple count events.
    assert!(counts >= 2, "expected ≥2 Count events, got {counts}");
}

#[test]
fn line_hook_fires_on_source_line_change() {
    let mut vm = fresh();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_LINE, 0);
    let _ = vm
        .eval("local a = 1\nlocal b = 2\nlocal c = a + b")
        .unwrap();
    let lines: Vec<u32> = EVENTS.with(|e| {
        e.borrow()
            .iter()
            .filter_map(|ev| match ev {
                RustHookEvent::Line(n) => Some(*n),
                _ => None,
            })
            .collect()
    });
    // 3-line source dispatches a Line event per line change.
    assert!(lines.len() >= 2, "expected ≥2 Line events, got {lines:?}");
}

#[test]
fn clear_rust_debug_hook_stops_events() {
    let mut vm = fresh();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_CALL, 0);
    let _ = vm.eval("local function f() return 1 end; f()").unwrap();
    let before = EVENTS.with(|e| e.borrow().len());
    EVENTS.with(|e| e.borrow_mut().clear());
    vm.clear_rust_debug_hook();
    let _ = vm.eval("local function g() return 2 end; g()").unwrap();
    let after = EVENTS.with(|e| e.borrow().len());
    assert!(before > 0);
    assert_eq!(
        after, 0,
        "events should not fire after clear_rust_debug_hook"
    );
}

#[test]
fn rust_hook_does_not_interfere_with_eval_result() {
    let mut vm = fresh();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_CALL | HOOK_MASK_RETURN, 0);
    let r = vm.eval("return 1 + 2").unwrap();
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Value::Int(3)));
}
