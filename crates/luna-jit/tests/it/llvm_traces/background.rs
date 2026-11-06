//! LLVM's code installed by the compile thread, in the background, while
//! the Vm runs Cranelift's.

use super::show;
use luna_jit::jit::trace::TraceTier;
use luna_jit::version::LuaVersion;

/// By default a hot trace moves to Cranelift's code, then, once it has run
/// that for a while, is compiled by LLVM on the compile thread, and the Vm
/// installs the LLVM code once it is ready.
#[test]
fn background_tier_up_installs_llvm_code() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend(&mut vm);
    vm.set_trace_tier(TraceTier::Auto);
    vm.set_trace_tier_up_at(64);
    vm.eval(
        "function RUN()
           local s = 0
           for i = 1, 20000 do s = s + (i * 3) % 7 end
           return s
         end",
    )
    .expect("defines RUN");
    let before = luna_jit::jit_backend::trace::llvm_codegen_count();
    let started = std::time::Instant::now();
    while luna_jit::jit_backend::trace::llvm_codegen_count() == before {
        let r = vm.eval("return RUN()").expect("runs");
        assert_eq!(show(&r), "Some(Int(60000))");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the LLVM code was never installed"
        );
    }
    let r = vm.eval("return RUN()").expect("runs on LLVM code");
    assert_eq!(show(&r), "Some(Int(60000))");
}

/// A loop entered once per call, waiting for LLVM's code, takes it at the
/// first entry after it is ready, not after many more calls.
#[test]
fn background_tier_up_reaches_a_loop_entered_once_per_call() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend_with(
        &mut vm,
        luna_jit::jit_backend::LlvmBackend {
            llvm_after: Some(std::time::Duration::ZERO),
        },
    );
    vm.set_trace_tier(TraceTier::Auto);
    vm.set_trace_tier_up_at(64);
    vm.eval(
        "function RUN()
           local s = 0
           for i = 1, 20000 do s = s + (i * 3) % 7 end
           return s
         end",
    )
    .expect("defines RUN");
    let before = luna_jit::jit_backend::trace::llvm_codegen_count();
    let mut calls = 0;
    while luna_jit::jit_backend::trace::llvm_codegen_count() == before {
        let r = vm.eval("return RUN()").expect("runs");
        assert_eq!(show(&r), "Some(Int(60000))");
        calls += 1;
        // LLVM's compile, behind the other tests' jobs on a loaded machine,
        // has 6 s; fewer calls than one ask in 64 entries would need
        assert!(calls < 60, "LLVM's code not taken after {calls} calls");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
