//! What the dispatcher checks before it enters a trace, checked when a side
//! trace's parent enters the trace the side trace links to: the slots it
//! reads, its checks before its loop, its frame, its move to the optimizing
//! tier.

use luna_core::__private::VmJitState;
use luna_jit::__private::ProtoTraceInternals;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::LuaClosure;
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

/// What a run left: the result as text, the links taken without the
/// dispatcher, the side cell fills by reason, and the chunk.
struct Ran {
    result: String,
    links: u64,
    fills: [u64; 8],
    chunk: Gc<LuaClosure>,
}

fn run_in(v: LuaVersion, src: &str, jit: bool, prep: impl FnOnce(&mut Vm)) -> (Ran, Vm) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    prep(&mut vm);
    let chunk = vm.load(src.as_bytes(), b"=checks").expect("load");
    let result = match vm.call_value(Value::Closure(chunk), &[]) {
        Ok(r) => r
            .iter()
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let c = vm.jit_counters_mut();
    let (links, fills) = (c.links, c.relink);
    let ran = Ran {
        result,
        links,
        fills,
        chunk,
    };
    (ran, vm)
}

/// `src` run with the trace JIT on, checked against the interpreter.
fn same(v: LuaVersion, src: &str, prep: impl Fn(&mut Vm)) -> (Ran, Vm) {
    let (want, _) = run_in(v, src, false, &prep);
    let (got, vm) = run_in(v, src, true, &prep);
    assert_eq!(got.result, want.result, "{v:?}");
    (got, vm)
}

/// A scan loop left for a copy loop through a side trace, the outer loop
/// round both 60 times; `pre` runs between the two loops, `body` is the
/// copy loop's.
fn two_loops(pre: &str, body: &str) -> String {
    format!(
        "local total = 0
         for rep = 1, 60 do
           local t = {{}}
           for i = 1, 300 do t[i] = i end
           local i = 1
           while i <= #t and t[i] < 200 do i = i + 1 end
           {pre}
           for j = i, #t do {body} end
         end
         return total"
    )
}

/// A slot only the linked trace reads, which the parent never checks:
/// once it holds another kind on the stack, the link is not taken (fewer
/// than when it keeps its kind) and the dispatcher does not enter the
/// linked trace either.
#[test]
fn a_slot_only_the_linked_trace_reads_is_checked_on_the_stack() {
    let with = |change: &str| {
        format!(
            "local x = 1
             {}",
            two_loops(change, "total = total + t[j] + x")
        )
    };
    let (kept, changed) = (with(""), with("if rep == 30 then x = '2' end"));
    for v in VERSIONS {
        let (k, _) = same(v, &kept, |_| {});
        let (c, _) = same(v, &changed, |_| {});
        assert!(c.links > 0, "{v:?}: no link taken");
        assert!(
            c.links < k.links,
            "{v:?}: {} links with the slot changed, {} kept",
            c.links,
            k.links
        );
    }
}

/// The side trace leaves a register of another kind than the linked trace
/// was entered with: the cell holds no link.
#[test]
fn a_side_trace_leaving_another_kind_links_to_nothing() {
    let src = two_loops(
        "local y = 1 if rep > 1 then y = 0.5 end",
        "total = total + t[j] * y",
    );
    for v in VERSIONS {
        let (r, _) = same(v, &src, |_| {});
        if v >= LuaVersion::Lua53 {
            assert!(r.fills[5] > 0, "{v:?}: no cell refused the link");
        }
    }
}

/// The side trace replaces `math.min`, which the linked trace folds,
/// checked once before its loop: entered from its entry, the linked trace
/// checks it again.
#[test]
fn a_side_trace_replacing_a_folded_function_is_seen_by_the_linked_trace() {
    let src = format!(
        "local mmin, plus = math.min, function(a, b) return a + b end
         {}",
        two_loops(
            "math.min = rep < 40 and mmin or plus",
            "total = total + math.min(t[j], 250)"
        )
    );
    for v in VERSIONS {
        let (r, _) = same(v, &src, |_| {});
        assert!(r.links > 0, "{v:?}: no link taken");
    }
}

/// `NEXT(_, k)`: `k + 1` up to 50, having marked the table in the global
/// `MARK` read-only when called with 0 once the global `ROUND` is 40.
fn next_marking(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_jit::vm::LuaError> {
    // 5.1 and 5.2 count in floats
    let (k, float) = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(k) => (k, false),
        Value::Float(k) => (k as i64, true),
        v => unreachable!("a number control variable: {v:?}"),
    };
    let round = vm.globals().get(Value::Str(vm.intern("ROUND")));
    if k == 0
        && matches!(round, Value::Int(40)) | matches!(round, Value::Float(r) if r == 40.0)
        && let Value::Table(t) = vm.globals().get(Value::Str(vm.intern("MARK")))
    {
        vm.set_readonly(t, true);
    }
    let next = match (k < 50, float) {
        (false, _) => Value::Nil,
        (true, false) => Value::Int(k + 1),
        (true, true) => Value::Float((k + 1) as f64),
    };
    Ok(vm.nat_return(fs, &[next]))
}

/// The side trace's first call of the linked loop's iterator, a host
/// function, marks read-only the table the linked trace stores into: the
/// linked trace, checking it before its loop, leaves, and the store raises
/// as in the interpreter.
#[test]
fn a_side_trace_marking_a_table_read_only_is_seen_by_the_linked_trace() {
    let src = "local total = 0 MARK = {}
               for rep = 1, 60 do
                 ROUND = rep
                 local t = {}
                 for i = 1, 300 do t[i] = i end
                 local i = 1
                 while i <= #t and t[i] < 200 do i = i + 1 end
                 for k in NEXT, nil, 0 do MARK[1] = k total = total + k end
               end
               return total";
    let prep = |vm: &mut Vm| {
        let f = vm.native(next_marking);
        vm.set_global("NEXT", f).expect("NEXT");
    };
    for v in VERSIONS {
        let (r, _) = same(v, src, prep);
        assert!(r.result.contains("readonly table"), "{v:?}: {}", r.result);
        assert!(r.links > 0, "{v:?}: no link taken");
    }
}

/// The linked trace calls a function it inlines: its window is wider than
/// the frame, whose registers past the frame the link clears as the
/// dispatcher does.
#[test]
fn a_linked_trace_inlining_a_call_runs_as_the_interpreter() {
    let src = format!(
        "local function twice(a) return a * 2 end
         {}",
        two_loops("", "total = total + twice(t[j])")
    );
    for v in VERSIONS {
        let (r, _) = same(v, &src, |_| {});
        assert!(r.links > 0, "{v:?}: no link taken");
    }
}

/// Once the linked trace moved to the optimizing tier, the links enter its
/// optimizing tier's entry.
#[test]
fn a_link_enters_the_moved_trace_at_its_new_entry() {
    let src = two_loops("", "total = total + t[j]");
    for v in VERSIONS {
        let (r, vm) = same(v, &src, |vm| vm.set_trace_tier_up_at(50));
        assert!(r.links > 0, "{v:?}: no link taken");
        let traces = r.chunk.proto.traces();
        let moved = (traces.iter())
            .filter(|t| t.current_entry() as usize != t.entry() as usize)
            .count();
        assert!(moved > 0, "{v:?}: no trace moved");
        drop(vm);
    }
}
