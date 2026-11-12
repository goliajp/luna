//! A side trace whose code calls nothing runs from its parent's exit
//! outside the compiler's scope; one that calls a helper, a function at an
//! address or another side trace from a side cell runs inside it. Each
//! kind runs, and the results are the interpreter's.

use luna_jit::__private::ProtoTraceInternals;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::heap::Gc;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `f(n)`'s loop leaves to `tail` before it returns; `f` is global.
fn src(tail: &str) -> String {
    format!(
        "T = {{1, 2, 3}} S = 'x' \
         function g(a) return a + 1 end \
         function f(n) local s = 0 for i = 1, n do s = s + i end {tail} return s end \
         local t = 0 for k = 1, 20000 do t = t + f(5) end return t"
    )
}

/// The side traces' code and what they call, by tail.
const TAILS: [(&str, bool); 4] = [
    // arithmetic only: no call
    ("s = s * 3 + n", false),
    // a table read through a helper
    ("s = s + T[n % 3 + 1]", true),
    // a string length through a helper
    ("s = s + #(S .. n)", true),
    // a Lua function call the trace leaves to the interpreter at, or calls
    ("s = s + g(n)", true),
];

fn vm(v: LuaVersion, jit: bool) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm
}

fn run(vm: &mut Vm, src: &str) -> String {
    let cl = vm.load(src.as_bytes(), b"=scope").expect("load");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => (r.iter().map(|v| format!("{v:?}")))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

fn proto_of_f(vm: &mut Vm) -> Gc<Proto> {
    let cl = vm.load(b"return f", b"=f").expect("load");
    match vm.call_value(Value::Closure(cl), &[]).as_deref() {
        Ok([Value::Closure(f)]) => f.proto,
        _ => panic!("no f"),
    }
}

#[test]
fn a_side_trace_runs_in_the_scope_only_when_its_code_calls_out() {
    for v in VERSIONS {
        for (tail, calls) in TAILS {
            let s = src(tail);
            let want = run(&mut vm(v, false), &s);
            let mut jit = vm(v, true);
            assert_eq!(run(&mut jit, &s), want, "{v:?} {tail}");
            let f = proto_of_f(&mut jit);
            let sides: Vec<bool> = (f.traces().iter())
                .filter(|t| t.is_side_trace())
                .map(|t| t.calls_helpers())
                .collect();
            if sides.is_empty() {
                continue;
            }
            assert!(
                sides.contains(&calls),
                "{v:?} {tail}: side traces calling out {sides:?}, want one {calls}"
            );
            assert!(
                jit.trace_side_trace_run_count() > 0,
                "{v:?} {tail}: no side trace ran"
            );
        }
    }
}
