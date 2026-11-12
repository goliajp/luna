//! A side trace recorded at an exit after which the function returns
//! closes at its first op: compiled, it would only return the pc it starts
//! at. It is neither compiled nor wired, and the exit is not recorded
//! again; the results stay the interpreter's.

use luna_core::__private::VmJitState;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `f`'s loop leaves to a `return`: the side trace at that exit is empty.
const RETURNS: &str = "local function f(n) local s = 0 for i = 1, n do s = s + i end return s end \
    local t = 0 for k = 1, 20000 do t = t + f(5) end return t";

/// `f`'s loop leaves to more work before the `return`: that side trace runs it.
const WORKS: &str = "local function f(n) local s = 0 for i = 1, n do s = s + i end \
    s = s * 3 + n return s end \
    local t = 0 for k = 1, 20000 do t = t + f(5) end return t";

fn vm(v: LuaVersion, jit: bool) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm
}

fn run(vm: &mut Vm, src: &str) -> String {
    let cl = vm.load(src.as_bytes(), b"=empty").expect("load");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => (r.iter().map(|v| format!("{v:?}")))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

#[test]
fn a_side_trace_that_only_returns_is_not_wired() {
    for v in VERSIONS {
        let want = run(&mut vm(v, false), RETURNS);
        let mut jit = vm(v, true);
        assert_eq!(run(&mut jit, RETURNS), want, "{v:?}");
        let c = &jit.jit().counters;
        assert_eq!(c.side_trace_empty, 1, "{v:?}: recorded once, not again");
        assert_eq!(c.side_trace_compiled, 0, "{v:?}");
        assert_eq!(jit.trace_side_trace_run_count(), 0, "{v:?}");
        assert!(c.dispatched > 10_000, "{v:?}: the loop's trace runs");
    }
}

#[test]
fn a_side_trace_that_runs_ops_is_wired() {
    for v in VERSIONS {
        let want = run(&mut vm(v, false), WORKS);
        let mut jit = vm(v, true);
        assert_eq!(run(&mut jit, WORKS), want, "{v:?}");
        let c = &jit.jit().counters;
        assert_eq!(c.side_trace_empty, 0, "{v:?}");
        assert!(c.side_trace_compiled > 0, "{v:?}");
        assert!(jit.trace_side_trace_run_count() > 10_000, "{v:?}");
    }
}
