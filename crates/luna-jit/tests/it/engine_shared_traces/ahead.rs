//! The first trace a Vm takes up for a function brings those of the
//! function's other loops, held until their heads are first reached.

use super::*;

/// Three loops of one function: the first runs `n1` times, the others `n2`
/// times; the second reads `k`'s length, which is a string's or a table's.
const LOOPS: &str = r#"
return function(n1, n2, k)
    local a = 0
    for i = 1, n1 do a = a + i end
    local s = 0
    for i = 1, n2 do s = s + #k end
    local c = 0
    for i = 1, n2 do c = c + i * 2 end
    return a, s, c
end
"#;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// What one call `f(n1, n2, k)` of [`LOOPS`] did in `vm`, `k` the string
/// "abc" or (`table`) the table {1, 2, 3}.
struct Call {
    results: String,
    compiled: u64,
    adopted: u64,
    dispatched: u64,
    held: u64,
    dropped: u64,
}

fn call(vm: &mut Vm, n1: u32, n2: u32, table: bool) -> Call {
    let k = if table { "{1, 2, 3}" } else { "'abc'" };
    let main = vm.load(LOOPS.as_bytes(), b"=loops").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    let wrap = format!("local f = ... return f({n1}, {n2}, {k})");
    let wrap = vm.load(wrap.as_bytes(), b"=call").expect("load");
    let before = (
        vm.trace_compiled_count(),
        vm.trace_adopted_count(),
        vm.trace_dispatched_count(),
        vm.jit().counters.ahead_held,
        vm.jit().counters.ahead_dropped,
    );
    let results = match vm.call_value(Value::Closure(wrap), &[Value::Closure(f)]) {
        Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
        Err(e) => format!("error: {}", vm.error_display(&e)),
    };
    Call {
        results,
        compiled: vm.trace_compiled_count() - before.0,
        adopted: vm.trace_adopted_count() - before.1,
        dispatched: vm.trace_dispatched_count() - before.2,
        held: vm.jit().counters.ahead_held - before.3,
        dropped: vm.jit().counters.ahead_dropped - before.4,
    }
}

#[test]
fn a_held_trace_runs_from_its_heads_first_back_edge() {
    for v in VERSIONS {
        let want = call(&mut interp(v), 50, 5, false).results;
        let engine = Engine::new();
        let a = call(&mut shared(&engine, v), 50, 50, false);
        assert!(
            a.compiled >= 3,
            "{v:?}: the first Vm compiled {}",
            a.compiled
        );
        // the second and third loops run 5 times, fewer than the 8 that
        // make a loop hot: only a trace held for them runs there
        let b = call(&mut shared(&engine, v), 50, 5, false);
        assert_eq!(b.results, want, "{v:?}");
        assert_eq!((b.compiled, b.held, b.dropped), (0, 2, 0), "{v:?}");
        assert!(b.adopted >= 3, "{v:?}: {} installed", b.adopted);
        assert!(b.dispatched >= 3, "{v:?}: {} entries", b.dispatched);
    }
}

#[test]
fn a_held_trace_that_does_not_admit_the_registers_is_dropped() {
    for v in VERSIONS {
        let want = call(&mut interp(v), 50, 50, true).results;
        let engine = Engine::new();
        let a = call(&mut shared(&engine, v), 50, 50, false);
        assert!(
            a.compiled >= 3,
            "{v:?}: the first Vm compiled {}",
            a.compiled
        );
        // the second loop's trace read a string: dropped where `k` is a
        // table, whose loop is then recorded here
        let b = call(&mut shared(&engine, v), 50, 50, true);
        assert_eq!(b.results, want, "{v:?}");
        assert_eq!((b.held, b.dropped), (2, 1), "{v:?}");
        assert!(b.compiled >= 1, "{v:?}: nothing recorded for the table");
        // a third Vm finds the first cached trace of the second loop held
        // and dropped again, then takes up the one the second Vm compiled
        // once that loop is hot
        let c = call(&mut shared(&engine, v), 50, 50, true);
        assert_eq!(c.results, want, "{v:?}");
        assert_eq!((c.compiled, c.held, c.dropped), (0, 2, 1), "{v:?}");
        assert!(c.adopted >= 3, "{v:?}: {} installed", c.adopted);
    }
}
