//! The dispatch loop head tests one flag for the instruction budget, the
//! memory cap and an armed hook. Each way of arming one while a chunk runs
//! must be seen from the next instruction on.

use std::cell::Cell;

use luna_core::runtime::Value;
use luna_core::runtime::value::NativeFn;
use luna_core::version::LuaVersion;
use luna_core::vm::exec::{HOOK_MASK_COUNT, RustHookEvent};
use luna_core::vm::{LuaError, Vm};

thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(0) };
}

fn count_hook(_vm: &mut Vm, ev: RustHookEvent) {
    if matches!(ev, RustHookEvent::Count) {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}

fn arm_rust_hook(vm: &mut Vm, _slot: u32, _nargs: u32) -> Result<u32, LuaError> {
    vm.set_rust_debug_hook(Some(count_hook), HOOK_MASK_COUNT, 1);
    Ok(0)
}

fn arm_budget(vm: &mut Vm, _slot: u32, _nargs: u32) -> Result<u32, LuaError> {
    vm.set_instr_budget(Some(1000));
    Ok(0)
}

fn arm_cap(vm: &mut Vm, _slot: u32, _nargs: u32) -> Result<u32, LuaError> {
    let cap = vm.memory_used() + 256 * 1024;
    vm.set_memory_cap(Some(cap));
    Ok(0)
}

fn vm_with(name: &str, f: NativeFn) -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let n = vm.heap.new_native(f, Box::new([]));
    vm.set_global(name, Value::Native(n)).expect("set_global");
    vm
}

fn message(e: &LuaError) -> String {
    match e.0 {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        v => format!("{v:?}"),
    }
}

fn int(r: Result<Vec<Value>, LuaError>) -> i64 {
    match r.expect("run")[0] {
        Value::Int(i) => i,
        v => panic!("expected an integer, got {v:?}"),
    }
}

#[test]
fn rust_hook_set_by_a_native_fires_on_the_next_instructions() {
    COUNT.with(|c| c.set(0));
    let mut vm = vm_with("arm", arm_rust_hook);
    vm.eval("arm() for i = 1, 100 do end").expect("run");
    let n = COUNT.with(Cell::get);
    assert!(n >= 100, "count hook fired {n} times");
}

#[test]
fn line_hook_keeps_firing_after_a_lua_hook_returns() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let src = "local lines = 0; debug.sethook(function() lines = lines + 1 end, 'l')\n\
               local a = 1\n\
               local b = 2\n\
               local c = 3\n\
               debug.sethook()\n\
               return lines";
    // PUC 5.1, 5.4 and 5.5 all count 4
    assert_eq!(int(vm.eval(src)), 4);
}

#[test]
fn a_coroutine_hook_fires_inside_the_resumed_thread() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let src = "local co = coroutine.create(function() local x = 1\n x = 2\n x = 3 end)\n\
               local n = 0; debug.sethook(co, function() n = n + 1 end, 'l')\n\
               coroutine.resume(co)\n\
               return n";
    // PUC 5.1, 5.4 and 5.5 all count 3
    assert_eq!(int(vm.eval(src)), 3);
}

#[test]
fn budget_set_by_a_native_is_enforced() {
    let mut vm = vm_with("arm", arm_budget);
    let r = vm.eval("arm() local i = 0 while i < 100000 do i = i + 1 end return i");
    let err = r.expect_err("the budget should stop the loop");
    assert!(
        message(&err).contains("instruction budget exceeded"),
        "{}",
        message(&err)
    );
}

#[test]
fn memory_cap_set_by_a_native_is_enforced() {
    let mut vm = vm_with("arm", arm_cap);
    let r = vm.eval("arm() local t = {} for i = 1, 100000 do t[i] = {} end return #t");
    let err = r.expect_err("the cap should stop the loop");
    assert!(
        message(&err).contains("memory cap exceeded"),
        "{}",
        message(&err)
    );
}

#[test]
fn a_hook_keeps_firing_after_resuming_an_unhooked_coroutine() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let src = "local co = coroutine.create(function() local y = 1 end)\n\
               local n = 0\n\
               debug.sethook(function() n = n + 1 end, 'l')\n\
               coroutine.resume(co)\n\
               local a = 1\n\
               local b = 2\n\
               debug.sethook()\n\
               return n";
    // PUC 5.1 to 5.5 all count 4
    assert_eq!(int(vm.eval(src)), 4);
}
