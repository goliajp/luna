//! `#` of a string inside a trace: the length comes from the string's
//! header, with the result the interpreter gives.

use luna_jit::version::LuaVersion;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

// short keys and a buffer that grows past the short-string limit and is
// cut back to the empty string
const SRC: &str = "local n, m, buf = 0, 0, ''
    for i = 1, 3000 do
        local k = 'item:' .. i .. ':' .. (i % 7)
        n = n + #k
        buf = buf .. k
        if #buf > 600 then m = m + #buf * i; buf = '' end
    end
    return n .. '/' .. m .. '/' .. #buf";

fn run(v: LuaVersion, trace: bool) -> (String, luna_jit::vm::Vm) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(trace);
    let r = vm.eval(SRC).unwrap();
    let s = match r[0] {
        luna_jit::runtime::Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{v:?}: expected a string, got {other:?}"),
    };
    (s, vm)
}

#[test]
fn string_length_in_a_trace_matches_the_interpreter() {
    for v in ALL {
        let (want, _) = run(v, false);
        let (got, vm) = run(v, true);
        assert_eq!(got, want, "{v:?}");
        assert!(
            vm.trace_compiled_count() >= 1 && vm.trace_dispatched_count() >= 1,
            "{v:?}: the loop must run in a trace; closed={} compiled={} failed={} \
             dispatched={} why={:?}",
            vm.trace_closed_count(),
            vm.trace_compiled_count(),
            vm.trace_compile_failed_count(),
            vm.trace_dispatched_count(),
            vm.trace_compile_failed_reasons(),
        );
        assert_eq!(vm.trace_compile_failed_count(), 0, "{v:?}");
    }
}
