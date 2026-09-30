//! A trace head whose recordings keep overflowing the recorder (more ops
//! than a trace may hold before it closes) is given up after a few tries,
//! like one whose traces keep failing to compile, instead of being
//! recorded again on every hot crossing.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

// `f`'s body is longer than a trace may be: every call past the call
// threshold would start a recording at its pc 0, and every back-edge of
// the loop one at the loop head that inlines `f`
fn src() -> String {
    let mut s = String::from("local function f(x)\n  local a = x\n");
    for i in 0..400 {
        s += &format!("  a = a + {i}\n");
    }
    s += "  return a\nend\nlocal s = 0\nfor i = 1, 300 do s = s + f(i) end\nreturn s\n";
    s
}

#[test]
fn overflowing_heads_stop_being_recorded() {
    let src = src();
    for v in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = luna_jit::new_with_jit(v);
        vm.jit.trace_hot_threshold = 1;
        vm.jit.call_hot_threshold = 1;
        let r = vm.eval(&src).expect("eval");
        assert!(
            matches!(r[0], Value::Int(23_985_150))
                || matches!(r[0], Value::Float(f) if f == 23_985_150.0),
            "{v:?}: {:?}",
            r[0]
        );
        let overflows = vm
            .jit
            .counters
            .close_cause_counts
            .get("trace-overflow")
            .copied()
            .unwrap_or(0);
        assert!(overflows > 0, "{v:?}: the body must overflow a recording");
        assert!(
            vm.trace_aborted_count() <= 6,
            "{v:?}: {} aborted recordings for two heads",
            vm.trace_aborted_count()
        );
    }
}
