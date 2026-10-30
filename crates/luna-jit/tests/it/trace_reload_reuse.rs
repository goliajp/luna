//! Code loaded again into the same Vm takes over the traces an earlier
//! load of the same content compiled, instead of recording and compiling
//! them again. Code that differs from it in one constant compiles its own.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const LOOP: &str = "local s = 0 for i = 1, 3000 do s = s + i * 3 end return s";

fn vm() -> luna_jit::vm::Vm {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm
}

fn run(vm: &mut luna_jit::vm::Vm, src: &str) -> i64 {
    match vm.eval(src).unwrap()[0] {
        Value::Int(n) => n,
        ref other => panic!("expected an integer, got {other:?}"),
    }
}

#[test]
fn a_reload_of_the_same_code_takes_over_its_trace() {
    let mut vm = vm();
    assert_eq!(run(&mut vm, LOOP), 3 * 4_501_500);
    let compiled = vm.trace_compiled_count();
    assert!(compiled >= 1);
    for _ in 0..3 {
        assert_eq!(run(&mut vm, LOOP), 3 * 4_501_500);
    }
    assert_eq!(
        vm.trace_compiled_count(),
        compiled,
        "a load of the same code must not compile its loop again"
    );
    assert!(vm.trace_adopted_count() >= 3);
    assert!(vm.trace_dispatched_count() >= 4);
}

#[test]
fn code_that_differs_in_one_constant_compiles_its_own_trace() {
    let mut vm = vm();
    assert_eq!(run(&mut vm, LOOP), 3 * 4_501_500);
    let compiled = vm.trace_compiled_count();
    assert_eq!(run(&mut vm, &LOOP.replace("i * 3", "i * 5")), 5 * 4_501_500);
    assert_eq!(vm.trace_adopted_count(), 0);
    assert!(vm.trace_compiled_count() > compiled);
}
