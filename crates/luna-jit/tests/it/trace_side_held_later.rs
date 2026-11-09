//! A side trace made while its parent trace runs, from a run of the
//! interpreter the parent's code calls into: its parent holds it only once
//! the outermost trace run ends, and the results are the interpreter's.

use luna_jit::__private::VmJitState;
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

/// `f`'s loop calls `cb` through `string.gsub` on every pass; with `NEST`
/// set, `cb` runs `f` again inside that call, where the loop takes the way
/// its trace leaves at, often enough for a side trace.
const SETUP: &str = "NEST, OUT = false, 0 \
    function cb() \
      if NEST then NEST = false; OUT = OUT + f(true, 2000) end \
      return 'y' \
    end \
    function f(nested, n) \
      local s = 0 \
      for i = 1, n do \
        if nested then s = s + 2 else s = s + 1 end \
        s = s + #string.gsub('x', 'x', cb) \
      end \
      return s \
    end \
    return f(false, 3000)";

const NESTED: &str = "NEST = true return f(false, 3000), OUT";

const AFTER: &str = "return f(true, 2000)";

fn run(vm: &mut Vm, src: &str) -> String {
    let cl = vm.load(src.as_bytes(), b"=held").expect("load");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => (r.iter().map(|v| format!("{v:?}")))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

fn vm(v: LuaVersion, jit: bool) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm
}

#[test]
fn a_side_trace_made_inside_its_parents_run_runs_after_it() {
    for v in VERSIONS {
        let mut interp = vm(v, false);
        let want: Vec<String> = [SETUP, NESTED, AFTER]
            .iter()
            .map(|s| run(&mut interp, s))
            .collect();
        let mut jit = vm(v, true);
        assert_eq!(run(&mut jit, SETUP), want[0], "{v:?}");
        assert_eq!(jit.trace_side_trace_compiled_count(), 0, "{v:?}");
        assert_eq!(run(&mut jit, NESTED), want[1], "{v:?}");
        // made inside the parent's run, held after it: none ran yet
        assert!(jit.jit().counters.held_later > 0, "{v:?}: none held later");
        assert!(jit.trace_side_trace_compiled_count() > 0, "{v:?}");
        assert_eq!(jit.trace_side_trace_run_count(), 0, "{v:?}");
        assert_eq!(run(&mut jit, AFTER), want[2], "{v:?}");
        assert!(
            jit.trace_side_trace_run_count() > 0,
            "{v:?}: the side trace never ran"
        );
    }
}
