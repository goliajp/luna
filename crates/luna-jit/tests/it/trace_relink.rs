//! Side traces the parent trace's code runs from its exits, going on in the
//! parent's loop when they end at its head.

use luna_jit::__private::ProtoTraceInternals;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::{LuaClosure, Proto};
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

/// What `src` returns, as text, with the trace JIT on or off, and the times
/// side traces went on in their parents' loops.
fn run(v: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let (r, n, _) = run_in(v, src, jit, |_| {});
    (r, n)
}

/// [`run`] on a Vm `prep` readies, also handing back the Vm.
fn run_in(
    v: LuaVersion,
    src: &str,
    jit: bool,
    prep: impl FnOnce(&mut Vm),
) -> (String, u64, (Vm, Gc<LuaClosure>)) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    prep(&mut vm);
    let cl = vm.load(src.as_bytes(), b"=relink").expect("load");
    let r = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => r
            .iter()
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let n = relinks(cl);
    (r, n, (vm, cl))
}

fn relinks(cl: Gc<LuaClosure>) -> u64 {
    fn walk(p: Gc<Proto>) -> u64 {
        let own: u64 = p.traces().iter().map(|t| t.side_relinks()).sum();
        own + p.protos.iter().map(|q| walk(*q)).sum::<u64>()
    }
    walk(cl.proto)
}

/// A loop whose body goes one of two ways: the trace takes one, the side
/// trace from the other's exit ends at the loop's head and goes on in the
/// trace's loop, with the results of the interpreter.
#[test]
fn a_side_trace_at_the_head_goes_on_in_the_loop() {
    let src = "local s, t = 0, 0 \
               for i = 1, 30000 do \
                 if i % 3 == 0 then s = s + i else t = t + 1 end \
               end \
               return s, t";
    for v in VERSIONS {
        let (want, _) = run(v, src, false);
        let (got, n) = run(v, src, true);
        assert_eq!(got, want, "{v:?}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
    }
}

/// Both ways, every dialect: the same results, and side traces going on in
/// their parents' loops.
fn same_and_relinked(src: &str) {
    for v in VERSIONS {
        let (want, _) = run(v, src, false);
        let (got, n) = run(v, src, true);
        assert_eq!(got, want, "{v:?}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
    }
}

/// A side trace replacing `math.min`, which its parent folds after checking
/// it once before the loop: going on in the loop, the parent checks it
/// again.
#[test]
fn a_side_trace_replacing_a_folded_function_is_seen() {
    same_and_relinked(
        "local lib, alt = math.min, function(a, b) return a + b end \
         local s = 0 \
         for i = 1, 30000 do \
           s = s + math.min(i, 3) \
           if i % 4 == 0 then if i % 8 == 0 then math.min = lib else math.min = alt end end \
         end \
         math.min = lib \
         return s",
    );
}

/// `NEXT(_, k)`, a generic-for iterator: `k + 1` up to 30000, having
/// marked the table in the global `MARK` read-only when called with 12000,
/// as a host function may.
fn next_marking(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_jit::vm::LuaError> {
    // 5.1 and 5.2 count in floats
    let (k, float) = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(k) => (k, false),
        Value::Float(k) => (k as i64, true),
        v => unreachable!("a number control variable: {v:?}"),
    };
    if k == 12000
        && let Value::Table(t) = vm.globals().get(Value::Str(vm.intern("MARK")))
    {
        vm.set_readonly(t, true);
    }
    let next = match (k < 30000, float) {
        (false, _) => Value::Nil,
        (true, false) => Value::Int(k + 1),
        (true, true) => Value::Float((k + 1) as f64),
    };
    Ok(vm.nat_return(fs, &[next]))
}

fn with_next(vm: &mut Vm) {
    let f = vm.native(next_marking);
    vm.set_global("NEXT", f).expect("NEXT");
}

/// A side trace whose iterator call, a host function, marks read-only the
/// table its parent stores into: going on in the loop, the parent's store
/// raises as in the interpreter. The side trace ends at the generic for's
/// back-edge, after the call, so each time it goes on in the loop it has
/// run the host function; `k == 12000` is on its path (`k % 4 == 0`).
///
/// In a side trace host code runs only there (a call, a metamethod or a
/// collection leaves it through its own exit), so its parent has the same
/// call in its loop and tests nothing read-only once before the loop.
#[test]
fn a_side_trace_marking_a_table_read_only_is_seen() {
    let src = "local t = {} MARK = t local s = 0 \
               for k in NEXT, nil, 0 do \
                 t[1] = k \
                 if k % 4 == 0 then s = s + 1 end \
               end \
               return t[1], s";
    for v in VERSIONS {
        let (want, ..) = run_in(v, src, false, with_next);
        let (got, n, _) = run_in(v, src, true, with_next);
        assert_eq!(got, want, "{v:?}");
        assert!(want.contains("readonly table"), "{v:?}: {want}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
    }
}

/// A side trace putting a read-only table in the register whose table its
/// parent tests once before the loop: going on in the loop, the parent
/// tests the new one and the store raises.
#[test]
fn a_side_trace_writing_a_register_the_loop_relies_on_is_seen() {
    let src = "local a = {} local sel, t = a, a \
               for i = 1, 30000 do \
                 t[1] = i \
                 if i == 25001 then sel = B end \
                 if i % 4 == 0 then t = sel end \
               end \
               return t[1]";
    let prep = |vm: &mut Vm| {
        let b = vm.eval("return {}").expect("B")[0];
        let Value::Table(bt) = b else { unreachable!() };
        vm.set_readonly(bt, true);
        vm.set_global("B", b).expect("B");
    };
    for v in VERSIONS {
        let (want, ..) = run_in(v, src, false, prep);
        let (got, n, _) = run_in(v, src, true, prep);
        assert_eq!(got, want, "{v:?}");
        assert!(want.contains("readonly table"), "{v:?}: {want}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
    }
}

/// A side trace writing a slot its parent never touches, whose exits leave
/// it on the stack: going on in the loop stores it there, so an exit of the
/// parent leaves the stack with the side trace's value.
#[test]
fn a_slot_only_the_side_trace_writes_reaches_the_stack() {
    same_and_relinked(
        "local s, u = 0, 0 \
         for i = 1, 30000 do \
           s = s + 1 \
           if i % 4 == 0 then u = u + i end \
           if i == 29999 then s = s + u end \
         end \
         return s, u",
    );
}

/// A side trace reading a slot its parent neither checks nor writes, which
/// holds a value of another type from some point: the exit checks the slot
/// before it runs the side trace, as the dispatcher does.
#[test]
fn a_side_trace_reading_an_unchecked_slot_checks_it() {
    same_and_relinked(
        "local s, u = 0, 1 \
         for i = 1, 30000 do \
           s = s + 1 \
           if i == 20001 then u = 0.5 end \
           if i % 4 == 0 then s = s + u end \
         end \
         return s",
    );
}

/// A side trace that leaves through one of its own exits after running from
/// its parent's code: the dispatcher decodes the return with the side
/// trace's exits.
#[test]
fn a_side_trace_leaving_by_its_own_exit_is_decoded_with_it() {
    same_and_relinked(
        "local s = 0 \
         for i = 1, 30000 do \
           if i % 4 == 0 then \
             if i % 1000 == 0 then s = s - 1 else s = s + 2 end \
           else s = s + 1 end \
         end \
         return s",
    );
}

/// A side trace the parent's code always runs and that always goes on in
/// the parent's loop moves to the optimizing tier: its cell then holds the
/// optimizing tier's entry.
#[test]
fn a_side_trace_always_going_on_in_the_loop_moves_up() {
    let src = "local s, t = 0, 0 \
               for i = 1, 200000 do \
                 if i % 2 == 0 then s = s + i else t = t + 1 end \
               end \
               return s, t";
    for v in VERSIONS {
        let (want, _) = run(v, src, false);
        let (got, n, (_vm, cl)) = run_in(v, src, true, |vm| vm.set_trace_tier_up_at(200));
        assert_eq!(got, want, "{v:?}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
        let states: Vec<(bool, bool)> = cl
            .proto
            .traces()
            .iter()
            .flat_map(|t| t.side_cell_states())
            .collect();
        assert!(
            states.iter().any(|&(held, up)| held && up),
            "{v:?}: no cell holds a side trace's optimizing tier: {states:?}"
        );
    }
}

/// A Vm taking over the traces another Vm of its engine compiled, their
/// side traces wired, runs the side traces from the parent's code with
/// cells of its own.
#[test]
fn a_vm_taking_traces_over_goes_on_in_the_loop() {
    let src = "return function() local s, t = 0, 0 \
               for i = 1, 30000 do if i % 3 == 0 then s = s + i else t = t + 1 end end \
               return s, t end";
    let engine = luna_jit::Engine::new();
    let call = |vm: &mut Vm| {
        vm.set_jit_enabled(false);
        let main = vm.load(src.as_bytes(), b"=t").expect("load");
        let Value::Closure(f) = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] else {
            panic!("no function");
        };
        let r: Vec<String> = (0..3)
            .map(|_| format!("{:?}", vm.call_value(Value::Closure(f), &[]).expect("call")))
            .collect();
        (r, relinks(f))
    };
    for v in VERSIONS {
        let mut plain = luna_jit::new_with_jit(v);
        plain.set_trace_jit_enabled(false);
        let (want, _) = call(&mut plain);
        let mut a = engine.new_vm(v);
        let (got_a, n_a) = call(&mut a);
        let mut b = engine.new_vm(v);
        let (got_b, n_b) = call(&mut b);
        assert_eq!(got_a, want, "{v:?}");
        assert_eq!(got_b, want, "{v:?}");
        assert!(n_a > 0, "{v:?}: the compiling Vm went on in no loop");
        assert!(b.trace_adopted_count() > 0, "{v:?}: nothing taken over");
        assert!(n_b > 0, "{v:?}: the Vm taking over went on in no loop");
    }
}

/// A side trace's own exit taken as often as any never starts a side trace
/// of its own: a side trace has none, so the parent's code calls side
/// traces one level deep at most.
#[test]
fn a_side_trace_has_no_side_traces() {
    use luna_jit::__private::CompiledTraceInternals;
    let src = "local s = 0 \
               for i = 1, 60000 do \
                 if i % 4 == 0 then \
                   if i % 8 == 0 then s = s + 2 else s = s + 3 end \
                 else s = s + 1 end \
               end \
               return s";
    for v in VERSIONS {
        let (want, _) = run(v, src, false);
        let (got, n, (_vm, cl)) = run_in(v, src, true, |_| {});
        assert_eq!(got, want, "{v:?}");
        assert!(n > 0, "{v:?}: no side trace went on in the loop");
        let traces = cl.proto.traces();
        let side = traces
            .iter()
            .filter(|t| t.dispatch_off_reason() == Some("side-trace"));
        let mut sides = 0;
        for t in side {
            sides += 1;
            let wired = (0..t.exit_hit_counts().len())
                .filter(|&i| t.has_side_trace(i))
                .count();
            assert_eq!(wired, 0, "{v:?}: a side trace with side traces");
        }
        assert!(sides > 0, "{v:?}: no side trace");
    }
}
