//! A loop whose recordings fail to compile is not recorded again by every
//! fresh load of the same chunk: the Vm remembers the head by its
//! function's content, and retries it once in a while.

use luna_jit::version::LuaVersion;

// string arithmetic does not lower, so every recording of this loop fails
const FAILING: &str = "local s, n = '1', 0
    for i = 1, 3000 do n = n + s end
    return n";

fn vm() -> luna_jit::vm::Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm
}

fn load_times(vm: &mut luna_jit::vm::Vm, src: &str, n: usize) {
    for _ in 0..n {
        vm.eval(src).unwrap();
    }
}

#[test]
fn reloads_of_a_failing_loop_record_it_a_bounded_number_of_times() {
    let mut vm = vm();
    load_times(&mut vm, FAILING, 1);
    // one prototype gives a head up after three failures
    assert_eq!(vm.trace_closed_count(), 3);
    assert_eq!(vm.trace_compile_failed_count(), 3);
    load_times(&mut vm, FAILING, 19);
    assert_eq!(
        vm.trace_closed_count(),
        3,
        "fresh loads of the same chunk must not record the head again"
    );
    // the 32nd load after the first gives it one more prototype's tries
    load_times(&mut vm, FAILING, 20);
    assert_eq!(vm.trace_closed_count(), 6);
    assert_eq!(vm.trace_compiled_count(), 0);
}

#[test]
fn other_code_is_recorded_as_usual() {
    let mut vm = vm();
    load_times(&mut vm, FAILING, 2);
    assert_eq!(vm.trace_closed_count(), 3);
    // another constant, another function: its head is its own
    load_times(&mut vm, &FAILING.replace("'1'", "'2'"), 1);
    assert_eq!(vm.trace_closed_count(), 6);
    // a loop that compiles is not held back by the failures
    load_times(
        &mut vm,
        "local n = 0 for i = 1, 3000 do n = n + i end return n",
        2,
    );
    assert!(vm.trace_compiled_count() >= 1);
}
