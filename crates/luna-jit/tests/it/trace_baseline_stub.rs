//! The baseline tier's code runs the side trace a filled side cell holds
//! through the shared call stub and goes on in its loop when the side
//! trace ends at its head; each Vm's code reads its own cells, also when
//! the code is taken up from another Vm.

use luna_jit::__private::{ProtoTraceInternals, VmJitState};
use luna_jit::Engine;
use luna_jit::jit::trace::TraceTier;
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

/// Past `lim` every pass leaves the loop's trace for the side trace, which
/// ends at the loop's head.
const SRC: &str = "
    return function(n, lim)
      local s = 0
      for i = 1, n do
        if i > lim then s = s + 1 else s = s + i * 2 end
      end
      return s
    end";

fn setup(mut vm: Vm, jit: bool) -> (Vm, Gc<LuaClosure>) {
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_tier(TraceTier::Baseline);
    let main = vm.load(SRC.as_bytes(), b"=stub").expect("load");
    let Value::Closure(f) = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] else {
        panic!("no function");
    };
    (vm, f)
}

/// `n` as `v` writes a number literal: 5.1 and 5.2 have floats only.
fn number(v: LuaVersion, n: i64) -> Value {
    match v {
        LuaVersion::Lua51 | LuaVersion::Lua52 => Value::Float(n as f64),
        _ => Value::Int(n),
    }
}

/// `f(n, lim)` in `vm` and in the interpreter's `it`, which must agree.
fn same(vm: (&mut Vm, Gc<LuaClosure>), it: (&mut Vm, Gc<LuaClosure>), n: i64, lim: i64) {
    let v = vm.0.version();
    let args = [number(v, n), number(v, lim)];
    let got = vm.0.call_value(Value::Closure(vm.1), &args).expect("call");
    let want = it.0.call_value(Value::Closure(it.1), &args).expect("call");
    assert_eq!(format!("{got:?}"), format!("{want:?}"), "{v:?}");
}

/// The passes that went on in the loop's trace's loop through a side trace
/// its code ran, and whether its side cell holds one.
fn root(f: Gc<LuaClosure>) -> (u64, bool) {
    let traces = f.proto.traces();
    let root = (traces.iter())
        .find(|t| !t.side_cell_states().is_empty())
        .expect("a trace with side cells");
    (root.side_relinks(), root.side_cell_states()[0].0)
}

/// The loop's trace (which has side cells): its baseline code's entry and
/// its first cell's address, once made.
fn code_and_cells(f: Gc<LuaClosure>) -> (usize, Option<usize>) {
    let traces = f.proto.traces();
    let t = traces.iter().find(|t| !t.side_cell_states().is_empty());
    let t = t.expect("a trace with side cells");
    (t.entry() as usize, t.side_cells_at().map(|c| c as usize))
}

/// A trace no side trace is wired to has no side cells made, and its code
/// is the one it was compiled with: its calling exits return as any exit.
#[test]
fn a_trace_without_side_traces_makes_no_cells_and_keeps_its_code() {
    for v in VERSIONS {
        let (mut it, g) = setup(luna_jit::new_with_jit(v), false);
        let (mut vm, f) = setup(luna_jit::new_with_jit(v), true);
        for _ in 0..20 {
            same((&mut vm, f), (&mut it, g), 500, 1 << 40);
        }
        assert!(code_and_cells(f).1.is_none(), "{v:?}: cells made");
        assert_eq!(root(f), (0, false), "{v:?}");
        let n = vm.jit().counters.calls_compiled;
        assert_eq!(n, 0, "{v:?}: code compiled again");
    }
}

/// Once the side trace fills the cell, the baseline code runs it and goes
/// on in its loop: the dispatcher sees the loop only a few times. Its code
/// is compiled again to call it, once.
#[test]
fn the_baseline_code_runs_a_filled_cells_side_trace_and_goes_on() {
    for v in VERSIONS {
        let (mut it, g) = setup(luna_jit::new_with_jit(v), false);
        let (mut vm, f) = setup(luna_jit::new_with_jit(v), true);
        same((&mut vm, f), (&mut it, g), 500, 1 << 40);
        let (first, _) = code_and_cells(f);
        same((&mut vm, f), (&mut it, g), 20_000, 5_000);
        assert_eq!(vm.jit().counters.calls_compiled, 1, "{v:?}");
        let (now, cells) = code_and_cells(f);
        assert_ne!(now, first, "{v:?}: the code was not replaced");
        assert!(cells.is_some(), "{v:?}: no cells made");
        assert_eq!(vm.trace_tiered_up_count(), 0, "{v:?}: a trace moved");
        let n = vm.trace_dispatched_count();
        assert!(n < 1_000, "{v:?}: {n} dispatches past the side trace");
        let (relinked, filled) = root(f);
        assert!(filled, "{v:?}: the cell holds no side trace");
        assert!(
            relinked > 14_000,
            "{v:?}: {relinked} passes went on in the loop"
        );
    }
}

fn shared(engine: &Engine) -> (Vm, Gc<LuaClosure>) {
    let mut vm = engine.new_vm(LuaVersion::Lua54);
    vm.set_trace_hot_threshold(8);
    vm.set_call_hot_threshold(8);
    setup(vm, true)
}

/// Two Vms run the same baseline code, the second taking it up from the
/// first: each runs the side trace only once its own cell holds one.
#[test]
fn vms_sharing_baseline_code_each_go_by_their_own_cells() {
    let engine = Engine::new();
    let (mut it, g) = setup(luna_jit::new_with_jit(LuaVersion::Lua54), false);
    let never = 1 << 40;
    let (mut a, fa) = shared(&engine);
    same((&mut a, fa), (&mut it, g), 500, never);
    let (mut b, fb) = shared(&engine);
    same((&mut b, fb), (&mut it, g), 5_000, 100);
    assert!(
        b.trace_adopted_count() > 0,
        "the second Vm compiled its own trace"
    );
    let (relinked, filled) = root(fb);
    assert!(filled, "the second Vm's cell holds no side trace");
    assert!(
        relinked > 4_000,
        "the second Vm's code went on {relinked} times"
    );
    assert_eq!(
        root(fa),
        (0, false),
        "the second Vm filled the first's cell"
    );
    assert!(code_and_cells(fa).1.is_none(), "the first Vm made cells");
    assert_eq!(
        a.jit().counters.calls_compiled,
        0,
        "the first Vm compiled its calls"
    );
    assert_eq!(b.jit().counters.calls_compiled, 1, "the second Vm's calls");
    same((&mut a, fa), (&mut it, g), 500, never);
    assert_eq!(
        root(fa),
        (0, false),
        "the first Vm ran the second's side trace"
    );
    same((&mut a, fa), (&mut it, g), 5_000, 100);
    let (relinked, filled) = root(fa);
    assert!(filled, "the first Vm's cell holds no side trace");
    assert_eq!(a.jit().counters.calls_compiled, 1, "the first Vm's calls");
    assert_ne!(
        code_and_cells(fa).1,
        code_and_cells(fb).1,
        "the Vms share cells"
    );
    assert!(
        relinked > 4_000,
        "the first Vm's code went on {relinked} times"
    );
}

/// A Vm that takes up the loop's trace together with its side trace runs
/// the code made with its calls from the start: nothing is compiled or
/// placed again once the side trace is wired.
#[test]
fn a_vm_taking_up_a_trace_with_its_side_trace_runs_its_calls_at_once() {
    let engine = Engine::new();
    let (mut it, g) = setup(luna_jit::new_with_jit(LuaVersion::Lua54), false);
    let (mut a, fa) = shared(&engine);
    same((&mut a, fa), (&mut it, g), 5_000, 100);
    assert_eq!(a.jit().counters.calls_compiled, 1, "the first Vm's calls");
    let (mut b, fb) = shared(&engine);
    same((&mut b, fb), (&mut it, g), 5_000, 100);
    let n = b.trace_adopted_count();
    assert!(n >= 2, "the second Vm took up {n} traces");
    assert_eq!(
        b.jit().counters.calls_compiled,
        0,
        "the second Vm compiled its calls again"
    );
    let (relinked, filled) = root(fb);
    assert!(filled, "the second Vm's cell holds no side trace");
    assert!(
        relinked > 4_000,
        "the second Vm's code went on {relinked} times"
    );
    assert_ne!(
        code_and_cells(fa).1,
        code_and_cells(fb).1,
        "the Vms share cells"
    );
}
