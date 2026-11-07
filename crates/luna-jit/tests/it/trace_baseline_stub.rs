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

/// Once the side trace fills the cell, the baseline code runs it and goes
/// on in its loop: the dispatcher sees the loop only a few times.
#[test]
fn the_baseline_code_runs_a_filled_cells_side_trace_and_goes_on() {
    for v in VERSIONS {
        let (mut it, g) = setup(luna_jit::new_with_jit(v), false);
        let (mut vm, f) = setup(luna_jit::new_with_jit(v), true);
        same((&mut vm, f), (&mut it, g), 20_000, 5_000);
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
    same((&mut a, fa), (&mut it, g), 500, never);
    assert_eq!(
        root(fa),
        (0, false),
        "the first Vm ran the second's side trace"
    );
    same((&mut a, fa), (&mut it, g), 5_000, 100);
    let (relinked, filled) = root(fa);
    assert!(filled, "the first Vm's cell holds no side trace");
    assert!(
        relinked > 4_000,
        "the first Vm's code went on {relinked} times"
    );
}
