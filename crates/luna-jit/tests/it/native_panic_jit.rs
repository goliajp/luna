//! Native panics while compiled code runs: luna-core's `panics.lua` (see
//! `luna-core/tests/native_panic/case.rs`) with the hot thresholds
//! at 2, under each trace tier and with the method JIT on. Its hot loops
//! call panicking natives from traces (a call that leaves the trace, a
//! generic-for iterator the trace calls through its helper, an `__index`
//! function), and the log must equal the interpreter's.

use luna_jit::jit::trace::TraceTier;
use luna_jit::vm::Vm;

#[path = "../../../luna-core/tests/native_panic/case.rs"]
mod core_case;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Trace(TraceTier, u32),
    MethodAndTrace,
}

const MODES: [Mode; 4] = [
    Mode::Trace(TraceTier::Baseline, 0),
    Mode::Trace(TraceTier::Optimizing, 0),
    Mode::Trace(TraceTier::Auto, 1),
    Mode::MethodAndTrace,
];

fn vm(v: luna_jit::version::LuaVersion, mode: Mode) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    match mode {
        Mode::Trace(tier, up_at) => {
            vm.set_jit_enabled(false);
            vm.set_trace_tier(tier);
            vm.set_trace_tier_up_at(up_at);
        }
        Mode::MethodAndTrace => {}
    }
    vm.jit.trace_hot_threshold = 2;
    vm.jit.call_hot_threshold = 2;
    vm
}

#[test]
fn native_panics_read_the_same_in_compiled_code() {
    let mut failures = Vec::new();
    for mode in MODES {
        for (dialect, v) in core_case::DIALECTS {
            let mut vm = vm(v, mode);
            let got = core_case::run(&mut vm);
            if got != core_case::expected(dialect) {
                failures.push(format!("{mode:?} == {dialect}\n{got}"));
            }
            if vm.trace_dispatched_count() == 0 {
                failures.push(format!(
                    "{mode:?} {dialect}: no trace ran (closed {} compiled {} aborted {} off {:?} failed {:?})",
                    vm.trace_closed_count(), vm.trace_compiled_count(), vm.trace_aborted_count(),
                    vm.trace_dispatch_off_reasons(), vm.trace_compile_failed_reasons()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
