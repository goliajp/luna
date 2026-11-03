//! A side trace that records its way back to the head of a compiled trace
//! in its own frame closes linked to it, going on through the numeric `for`
//! loop edges it recorded; each such edge leaves for the interpreter when
//! the loop goes the way the recording did not, and the results are the
//! interpreter's in every dialect.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run(v: LuaVersion, src: &str, jit: bool) -> (Vec<Value>, u64, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let r = vm.eval(src).unwrap();
    let dropped = vm
        .trace_close_cause_counts()
        .get("reached-compiled-trace")
        .copied()
        .unwrap_or(0);
    (r, dropped, vm.trace_closed_count())
}

/// The results with the trace JIT on and off, compared, and the
/// recordings dropped at a compiled trace's head with it on.
fn check(src: &str) -> Vec<u64> {
    DIALECTS
        .iter()
        .map(|&v| {
            let (want, _, _) = run(v, src, false);
            let (got, dropped, closed) = run(v, src, true);
            assert_eq!(format!("{got:?}"), format!("{want:?}"), "{v:?}: {src}");
            assert!(
                closed >= 2,
                "{v:?}: a root and a side trace close, got {closed}"
            );
            dropped
        })
        .collect()
}

// the inner loop's trace leaves when its loop ends; the side trace from
// there runs the rest of the outer body, goes on through the outer loop's
// edge and back to the inner loop's head, and leaves at the outer loop's
// end on the last pass
#[test]
fn an_outer_loop_edge_goes_on_and_leaves_when_the_loop_ends() {
    let src = "local s, t = 0, 0
        for i = 1, 300 do
            for j = 1, 40 do s = s + j end
            t = t + i * 2
        end
        return s, t";
    assert_eq!(check(src), [0; 5]);
}

// a short loop in the outer body ran out while it was recorded: the side
// trace goes on past it, and leaves for its body when it has more to go
#[test]
fn a_loop_recorded_ending_leaves_when_it_goes_on() {
    let src = "local s, t = 0, 0
        for i = 1, 300 do
            for j = 1, 40 do s = s + j end
            for k = 1, i % 3 do t = t + k end
            t = t + 1
        end
        return s, t";
    assert_eq!(check(src), [0; 5]);
}

// a float loop and a negative integer step take the same way
#[test]
fn float_and_downward_loops_step_as_the_interpreter() {
    for src in [
        "local s, t = 0, 0
         for x = 0.5, 150.5, 0.5 do
             for j = 1, 40 do s = s + j end
             t = t + x
         end
         return s, t",
        "local s, t = 0, 0
         for i = 300, 1, -1 do
             for j = 1, 40 do s = s + j end
             t = t + i
         end
         return s, t",
    ] {
        assert_eq!(check(src), [0; 5]);
    }
}
