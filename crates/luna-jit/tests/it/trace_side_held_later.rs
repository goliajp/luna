//! A side trace made while its parent trace runs, from a finalizer a
//! collection step inside the parent's code runs: its parent holds it only
//! once the outermost trace run ends, and the results are the
//! interpreter's.

use luna_jit::__private::{ProtoTraceInternals, VmJitState};
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

/// `f`'s loop makes a table on every pass, kept a few passes so that it
/// is made, not sunk, and collection steps fall due inside its trace.
/// `arm` leaves an object whose finalizer runs `f` again from such a step,
/// where the loop takes the way its trace leaves at, often enough for a
/// side trace.
const SETUP: &str = "OUT, K = 0, {} \
    function f(nested, n) \
      local s = 0 \
      for i = 1, n do \
        local t = {i} \
        K[i % 4 + 1] = t \
        if nested then s = s + 2 else s = s + t[1] % 3 end \
      end \
      return s \
    end \
    function arm() \
      local fin = function() OUT = OUT + f(true, 3000) end \
      if newproxy then getmetatable(newproxy(true)).__gc = fin \
      else setmetatable({}, {__gc = fin}) end \
    end \
    return f(false, 3000)";

const AFTER: &str = "collectgarbage() return f(true, 3000), OUT";

/// What `src` returns, as text.
fn run(vm: &mut Vm, src: &str) -> String {
    let cl = vm.load(src.as_bytes(), b"=held").expect("load");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => (r.iter().map(|v| format!("{v:?}")))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

/// The prototype of the global `f`, which the global keeps alive.
fn proto_of_f(vm: &mut Vm) -> Gc<Proto> {
    let cl = vm.load(b"return f", b"=f").expect("load");
    match vm.call_value(Value::Closure(cl), &[]).as_deref() {
        Ok([Value::Closure(f)]) => f.proto,
        _ => panic!("no f"),
    }
}

/// The side traces run: by the dispatcher at their parents' exits, and by
/// the parents' code (each pass that went on in a parent's loop) of `p`
/// and the prototypes in it.
fn side_runs(vm: &Vm, p: Gc<Proto>) -> u64 {
    fn relinks(p: Gc<Proto>) -> u64 {
        let own: u64 = p.traces().iter().map(|t| t.side_relinks()).sum();
        own + p.protos.iter().map(|q| relinks(*q)).sum::<u64>()
    }
    vm.trace_side_trace_run_count() + relinks(p)
}

fn vm(v: LuaVersion, jit: bool) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm
}

/// Whether the finalizer runs inside the trace depends on where the
/// collection's steps fall, which the loop's length moves: the lengths
/// tried, until one makes the side trace inside the parent's run.
const LENGTHS: [u32; 8] = [
    200_000, 150_000, 250_000, 120_000, 300_000, 90_000, 350_000, 400_000,
];

#[test]
fn a_side_trace_made_inside_its_parents_run_runs_after_it() {
    for v in VERSIONS {
        let made_inside = LENGTHS.iter().any(|&n| made_inside_and_ran_after(v, n));
        assert!(
            made_inside,
            "{v:?}: no side trace made inside its parent's run"
        );
    }
}

/// With the loop `n` long: the results are the interpreter's; when the side
/// trace was made inside its parent's run, none ran until that run ended,
/// and it runs after (`true` then).
fn made_inside_and_ran_after(v: LuaVersion, n: u32) -> bool {
    let nested = format!("arm() return f(false, {n})");
    let srcs = [SETUP, nested.as_str(), AFTER];
    let mut interp = vm(v, false);
    let want: Vec<String> = srcs.iter().map(|s| run(&mut interp, s)).collect();
    let mut jit = vm(v, true);
    assert_eq!(run(&mut jit, SETUP), want[0], "{v:?}");
    let f = proto_of_f(&mut jit);
    assert_eq!(jit.trace_side_trace_compiled_count(), 0, "{v:?}");
    assert_eq!(run(&mut jit, &nested), want[1], "{v:?} {n}");
    let inside = jit.jit().counters.held_later > 0;
    if inside {
        // made inside the parent's run, held after it: none ran yet
        assert!(jit.trace_side_trace_compiled_count() > 0, "{v:?} {n}");
        assert_eq!(side_runs(&jit, f), 0, "{v:?} {n}");
    }
    assert_eq!(run(&mut jit, AFTER), want[2], "{v:?} {n}");
    if inside {
        assert!(
            side_runs(&jit, f) > 0,
            "{v:?} {n}: the side trace never ran"
        );
    }
    inside
}
