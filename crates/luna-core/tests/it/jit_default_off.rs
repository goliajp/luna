//! A Vm with no JIT backend installed (`Vm::new`, the sandbox builder)
//! keeps both JIT flags off: nothing could be compiled, so no hot counter
//! ticks and the recorder never starts. Installing a backend turns on the
//! flags the embedder has not set.

use luna_core::jit::NullJitBackend;
use luna_core::runtime::{LuaClosure, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

// a while loop (Jmp back-edge), a numeric for (ForLoop back-edge) and a
// function called often enough to pass the call threshold
const HOT: &str = r#"
    local function f(x) return x + 1 end
    local s, i = 0, 0
    while i < 500 do i = i + 1; s = s + i end
    for j = 1, 500 do s = f(s) + j end
    return s, f
"#;

fn run(
    vm: &mut Vm,
) -> (
    luna_core::runtime::Gc<LuaClosure>,
    luna_core::runtime::Gc<LuaClosure>,
) {
    let main = vm.load(HOT.as_bytes(), b"=hot").expect("load");
    let rets = vm.call_value(Value::Closure(main), &[]).expect("run");
    assert!(matches!(rets[0], Value::Int(251_000)), "{:?}", rets[0]);
    let Value::Closure(f) = rets[1] else {
        panic!("expected the function back, got {:?}", rets[1]);
    };
    (main, f)
}

fn assert_idle(mut vm: Vm) {
    assert!(!vm.jit_enabled());
    assert!(!vm.trace_jit_enabled());
    let (main, f) = run(&mut vm);
    assert_eq!(main.proto.trace_hot_count.get(), 0);
    assert_eq!(main.proto.call_hot_count.get(), 0);
    assert_eq!(f.proto.call_hot_count.get(), 0);
    assert_eq!(vm.trace_closed_count(), 0);
    assert_eq!(vm.trace_aborted_count(), 0);
    assert_eq!(vm.trace_compile_failed_count(), 0);
}

#[test]
fn default_vm_keeps_the_jit_idle() {
    assert_idle(Vm::new(LuaVersion::Lua54));
    assert_idle(Vm::new_minimal(LuaVersion::Lua54));
}

#[test]
fn sandbox_vm_keeps_the_jit_idle() {
    assert_idle(Vm::sandbox(LuaVersion::Lua54).open_base().build());
}

#[test]
fn installing_a_backend_turns_the_jit_on() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.install_jit_backend(NullJitBackend, NullJitBackend);
    assert!(vm.jit_enabled());
    assert!(vm.trace_jit_enabled());
    let (main, _) = run(&mut vm);
    assert!(main.proto.trace_hot_count.get() > 0);
    assert!(vm.trace_closed_count() > 0);
}

#[test]
fn a_flag_set_before_install_is_kept() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(false);
    vm.install_jit_backend(NullJitBackend, NullJitBackend);
    assert!(vm.jit_enabled());
    assert!(!vm.trace_jit_enabled());
    let (main, _) = run(&mut vm);
    assert_eq!(main.proto.trace_hot_count.get(), 0);
    assert_eq!(vm.trace_closed_count(), 0);

    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.install_null_jit();
    vm.install_jit_backend(NullJitBackend, NullJitBackend);
    assert!(!vm.jit_enabled());
    assert!(!vm.trace_jit_enabled());
}

fn eval_async_to_end(vm: &mut Vm) {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let mut fut =
        std::pin::pin!(vm.eval_async("local s = 0 for i = 1, 100 do s = s + i end return s"));
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(r) = fut.as_mut().poll(&mut cx) {
            assert!(matches!(r.expect("eval_async")[0], Value::Int(5050)));
            return;
        }
    }
}

#[test]
fn eval_async_before_install_leaves_the_default() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    eval_async_to_end(&mut vm);
    assert!(!vm.jit_enabled());
    vm.install_jit_backend(NullJitBackend, NullJitBackend);
    assert!(vm.jit_enabled());
    assert!(vm.trace_jit_enabled());

    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    eval_async_to_end(&mut vm);
    vm.install_jit_backend(NullJitBackend, NullJitBackend);
    assert!(!vm.jit_enabled());
}
