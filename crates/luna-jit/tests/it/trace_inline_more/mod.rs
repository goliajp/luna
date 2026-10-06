//! Traces through more kinds of inlined calls: side traces started inside
//! inlined functions, vararg callees, calls that want several results or
//! pass a variable number of arguments, and closures created in inlined
//! functions. Each program runs under the interpreter and under both trace
//! tiers (and the move from one to the other); the results must agree, and
//! the counters show the new paths ran in traces.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

mod calls;
mod closures;
mod side_traces;

pub(super) fn interp(version: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

pub(super) fn traced(version: LuaVersion, tier: TraceTier, tier_up_at: u32) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    // the method JIT would run whole functions instead of the interpreter
    // the trace recorder watches
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm.jit.call_hot_threshold = 8;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

/// `src` returns a function; the results of calling it `calls` times.
pub(super) fn results(vm: &mut Vm, src: &str, calls: usize) -> Vec<String> {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    (0..calls)
        .map(|_| match vm.call_value(Value::Closure(f), &[]) {
            Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect()
}

fn show(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        v => format!("{v:?}"),
    }
}

pub(super) const TIERS: [(&str, TraceTier, u32); 4] = [
    ("baseline", TraceTier::Baseline, 0),
    ("cranelift", TraceTier::Optimizing, 0),
    ("tier up at once", TraceTier::Auto, 1),
    ("tier up partway", TraceTier::Auto, 300),
];

/// Runs `src` in every tier against the interpreter of `version`; returns
/// `count` of each tier's Vm after the run.
pub(super) fn agree_in<T>(
    version: LuaVersion,
    src: &str,
    calls: usize,
    count: impl Fn(&Vm) -> T,
) -> Vec<T> {
    let want = results(&mut interp(version), src, calls);
    TIERS
        .iter()
        .map(|&(name, tier, at)| {
            let mut vm = traced(version, tier, at);
            let got = results(&mut vm, src, calls);
            assert_eq!(got, want, "{name}");
            count(&vm)
        })
        .collect()
}

/// [`agree_in`] under 5.4.
pub(super) fn agree<T>(src: &str, calls: usize, count: impl Fn(&Vm) -> T) -> Vec<T> {
    agree_in(LuaVersion::Lua54, src, calls, count)
}

/// Asserts `count` of every tier's Vm is above zero.
pub(super) fn assert_every_tier(src: &str, calls: usize, what: &str, count: impl Fn(&Vm) -> u64) {
    for (k, n) in agree(src, calls, count).into_iter().enumerate() {
        assert!(n > 0, "{}: {what} 0", TIERS[k].0);
    }
}
