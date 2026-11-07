//! The dispatcher runs a side trace whose register its parent took
//! unchecked on entry but wrote by the exit: the value is in the parent's
//! registers, not yet on the stack, so the stack's stale value does not
//! keep the side trace from running.

use luna_core::__private::VmJitState;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `v` is nil on the stack whenever the trace is entered after an array
/// read that missed, and the parent writes it before the exit at
/// `i % 7 == 0`, whose side trace reads it. The side trace inlines `h`,
/// so its window is wider than its parent's: the parent's code leaves it
/// to the dispatcher.
const SRC: &str = "
    return function()
        local t = {1, 2, 3, 4, 5, 6, 7, 8}
        local s = 0
        local function h(x) return x * 2 end
        for i = 1, 3000 do
            local v = t[i % 10 + 1]
            if v then
                s = s + v
                if i % 7 == 0 then s = s + v + h(i) end
            end
        end
        return s
    end";

/// The results with the trace JIT on or off, and with it on, the side
/// trace runs and the cell fills of the last call.
fn run(v: LuaVersion, jit: bool) -> (Vec<Value>, u64, [u64; 6]) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let main = vm.load(SRC.as_bytes(), b"=held").expect("load");
    let Value::Closure(f) = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] else {
        panic!("no function");
    };
    for _ in 0..3 {
        vm.call_value(Value::Closure(f), &[]).expect("call");
    }
    let runs = vm.trace_side_trace_run_count();
    let r = vm.call_value(Value::Closure(f), &[]).expect("call");
    let runs = vm.trace_side_trace_run_count() - runs;
    (r, runs, vm.jit_counters_mut().relink)
}

#[test]
fn a_register_the_exit_wrote_is_not_checked_on_the_stack() {
    // the passes that take the exit: `i % 7 == 0` with a read that hits
    let exits = (1..=3000u32).filter(|i| i % 7 == 0 && i % 10 < 8).count() as u64;
    for v in VERSIONS {
        let (want, ..) = run(v, false);
        let (got, runs, fills) = run(v, true);
        assert_eq!(format!("{got:?}"), format!("{want:?}"), "{v:?}");
        assert_eq!(
            runs, exits,
            "{v:?}: the side trace did not run at every exit ({fills:?})"
        );
    }
}
