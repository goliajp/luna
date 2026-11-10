//! A loop whose trace refuses the entry types it is reached with: the
//! refused entries do not move the trace to the optimizing tier, the fast
//! loop runs on past the head while entries keep failing the same way, and
//! the trace is entered again once they pass, with the interpreter's
//! results throughout.

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

/// A vararg function (left to the interpreter by the method JIT) whose loop
/// gets its trace from integer entries; `{phases}` then calls it on.
fn program(phases: &str) -> String {
    format!(
        "local function f(x, n, ...) local s = x + select('#', ...) \
         for i = 1, n do s = s + i end return s end \
         local r = {{f(0, 2000)}} \
         {phases} \
         return table.concat(r, ' ')"
    )
}

#[derive(Debug, PartialEq)]
struct Ran {
    result: String,
    dispatched: u64,
    tiered_up: u64,
}

fn run(v: LuaVersion, src: &str, jit: bool) -> Ran {
    let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, 1);
    luna_jit::install_default_jit(&mut vm);
    vm.open_all_libs().expect("the libraries open");
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_tier_up_at(500);
    let cl = vm.load(src.as_bytes(), b"=refused").expect("load");
    let result = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let c = vm.jit_counters_mut();
    Ran {
        result,
        dispatched: c.dispatched,
        tiered_up: c.tiered_up,
    }
}

#[test]
fn refused_entries_neither_dispatch_nor_move_the_trace_and_passing_ones_enter_again() {
    let prime = program("");
    let float = program("r[#r + 1] = f(0.5, 20000)");
    let back = program("r[#r + 1] = f(0.5, 20000) r[#r + 1] = f(0, 20000)");
    for v in VERSIONS {
        let base = run(v, &prime, true);
        let refused = run(v, &float, true);
        let again = run(v, &back, true);
        assert_eq!(base.result, run(v, &prime, false).result, "{v:?}");
        assert_eq!(refused.result, run(v, &float, false).result, "{v:?}");
        assert_eq!(again.result, run(v, &back, false).result, "{v:?}");
        // from 5.3 on a float is not an integer entry: every entry of the
        // float call is refused
        if v >= LuaVersion::Lua53 {
            assert_eq!(refused.dispatched, base.dispatched, "{v:?}: {refused:?}");
            assert_eq!(refused.tiered_up, base.tiered_up, "{v:?}: {refused:?}");
            assert!(again.dispatched > refused.dispatched, "{v:?}: {again:?}");
        }
    }
}

#[test]
fn a_boolean_entry_passes_for_either_value_after_a_refusal() {
    // the trace is entered with `b` false; a string in `b` is refused, and
    // `true` passes as the boolean entry does
    let src = "local function f(b, n, ...) local s = 0 \
               for i = 1, n do if b then s = s + 2 else s = s + 1 end end return s end \
               local r = {f(false, 2000), f('x', 5000), f(true, 5000), f(false, 5000)} \
               return table.concat(r, ' ')";
    for v in VERSIONS {
        let jit = run(v, src, true);
        assert_eq!(jit.result, run(v, src, false).result, "{v:?}");
    }
}
