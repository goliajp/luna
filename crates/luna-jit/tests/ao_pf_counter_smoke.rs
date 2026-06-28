//! v2.0 Phase 5 Track AO sub-track AO-PF — in-process smoke for the
//! `trace_materialize_frames_fires` counter. Confirms the counter
//! wiring is sound: a JIT-mode fib(28) run that exercises the inline
//! self-rec dispatch path must leave the counter > 0.

use luna_jit::jit_backend::trace_materialize_frames_fires;
use luna_jit::version::LuaVersion;

#[test]
fn counter_increments_on_fib_inline_self_rec_dispatch() {
    let before = trace_materialize_frames_fires();

    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(28)",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(317811)));
    assert!(vm.trace_compiled_count() >= 1, "fib's trace must compile");
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib(28) must dispatch via inline self-rec"
    );

    let after = trace_materialize_frames_fires();
    let delta = after - before;
    eprintln!(
        "AO-PF counter: before={before} after={after} delta={delta} \
         trace_compiled={} trace_dispatched={}",
        vm.trace_compiled_count(),
        vm.trace_dispatched_count()
    );
    assert!(
        delta > 0,
        "expected trace_materialize_frames_fires counter to increment when \
         fib(28) dispatches via inline self-rec path; delta={delta}"
    );
}
