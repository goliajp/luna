//! P15-A v1 — dispatcher side-trace TRIGGER. When a side-exit's
//! `exit_hit_counts` slot crosses `HOTEXIT_THRESHOLD` while no
//! recording is active, the dispatcher starts a fresh
//! `TraceRecord::start_side_trace` with `side_trace_parent`
//! metadata. The recording's compile/link wiring lands in v2; v1
//! only verifies the TRIGGER fires.
//!
//! Targets (per `e52e67a` P15-prep probe data):
//! - fib_28 idx 8 = 237877 hits (≫ threshold 10) → must trigger
//! - binary_trees_d4 idx 6 = 988 hits → must trigger
//! - concat_str_for_10k: 1 hit total → must NOT trigger
//!
//! v1 is observability-only:`trace_side_trace_started_count` must
//! be > 0 on positive cases and == 0 on the negative case.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// fib(28) — multiple inline cmp@d>0 side-exits accumulate hits
/// far past threshold. The first side-trace start MUST fire by the
/// time fib(28) finishes.
#[test]
fn fib_28_triggers_side_trace() {
    let mut vm = Vm::new(LuaVersion::Lua55);
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
    assert!(matches!(r[0], luna::runtime::Value::Int(317811)));
    assert!(
        vm.trace_side_trace_started_count() >= 1,
        "fib_28 must trigger at least one side-trace start; got {}",
        vm.trace_side_trace_started_count()
    );
}

/// binary_trees_d4 — at least one hot exit crosses threshold;
/// trigger must fire.
#[test]
fn binary_trees_d4_triggers_side_trace() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function make(d)
                 if d == 0 then return 1 end
                 return 1 + make(d-1) + make(d-1)
             end
             local s = 0
             for i = 1, 200 do s = s + make(4) end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(6200)));
    assert!(
        vm.trace_side_trace_started_count() >= 1,
        "binary_trees_d4 must trigger at least one side-trace start; got {}",
        vm.trace_side_trace_started_count()
    );
}

/// concat_str_for_10k — single dispatch, no exit reaches threshold.
/// Side-trace trigger must NOT fire. Also guards against accidental
/// trigger on every per_exit_tags lookup (the per_exit_tags array is
/// non-empty for this trace, but its hit counts cap at 1).
#[test]
fn concat_str_does_not_trigger_side_trace() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for i = 1, 10000 do t[i] = 'x' end
             local function join(tt)
                 local s = ''
                 for _, v in ipairs(tt) do s = s..v end
                 return s
             end
             return string.len(join(t))",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(10000)));
    assert_eq!(
        vm.trace_side_trace_started_count(),
        0,
        "concat_str_for_10k must NOT trigger side-trace start \
         (no exit hits threshold); got {}",
        vm.trace_side_trace_started_count()
    );
}

/// Trace JIT disabled — side-trace trigger must NOT fire even on
/// fib_28-shape workloads. Guards against the gate being bypassed.
#[test]
fn side_trace_skipped_when_trace_jit_disabled() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(20)",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(6765)));
    assert_eq!(
        vm.trace_side_trace_started_count(),
        0,
        "trace JIT disabled → no dispatch → no side-trace trigger; got {}",
        vm.trace_side_trace_started_count()
    );
}
