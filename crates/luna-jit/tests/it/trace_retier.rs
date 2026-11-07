//! The optimizing tier's code calls the side traces of the exits whose
//! side cells held one when it was compiled; the others leave to the
//! dispatcher. A cell filled after the move has the trace compiled again,
//! and a Vm taking code up from another compiles its own when its cells
//! differ from those the code was compiled for.

use luna_jit::__private::{ProtoTraceInternals, VmJitState};
use luna_jit::Engine;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::LuaClosure;
use luna_jit::runtime::heap::Gc;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const AT: u32 = 1000;

/// Past `lim` every pass leaves the loop's trace for the side trace, which
/// ends at the loop's head with the registers of the kinds the loop takes,
/// so that it goes on in the loop.
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
    vm.set_trace_tier_up_at(AT);
    let main = vm.load(SRC.as_bytes(), b"=retier").expect("load");
    let Value::Closure(f) = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] else {
        panic!("no function");
    };
    (vm, f)
}

fn call(vm: &mut Vm, f: Gc<LuaClosure>, n: i64, lim: i64) -> String {
    let args = [Value::Int(n), Value::Int(lim)];
    format!(
        "{:?}",
        vm.call_value(Value::Closure(f), &args).expect("call")
    )
}

/// Calls `f(n, lim)` in `vm` and the interpreter's copy in `it`, checking
/// they return the same.
fn same(vm: (&mut Vm, Gc<LuaClosure>), it: (&mut Vm, Gc<LuaClosure>), n: i64, lim: i64) {
    assert_eq!(call(vm.0, vm.1, n, lim), call(it.0, it.1, n, lim));
}

/// The side traces the loop's trace went on with in its loop from its
/// code, and whether its optimizing tier's loop exists.
fn root(f: Gc<LuaClosure>) -> (u64, bool) {
    let traces = f.proto.traces();
    let root = (traces.iter())
        .find(|t| !t.side_cell_states().is_empty())
        .expect("a trace with side cells");
    let moved = (root.tier_up())
        .is_some_and(|t| !luna_jit::__private::tier_up_loop_cell(t).get().is_null());
    (root.side_relinks(), moved)
}

/// The trace moves with its side cell empty; once the side trace fills it,
/// the trace is compiled again, once, and the new code goes on in its loop
/// through the side trace.
#[test]
fn a_cell_filled_after_the_move_compiles_the_trace_again() {
    let (mut it, g) = setup(luna_jit::new_with_jit(LuaVersion::Lua54), false);
    let (mut vm, f) = setup(luna_jit::new_with_jit(LuaVersion::Lua54), true);
    same((&mut vm, f), (&mut it, g), 20_000, 5_000);
    let n = vm.trace_dispatched_count();
    assert!(
        n < 1_000,
        "{n} dispatches: the moved code left the side trace to the dispatcher"
    );
    let c = &vm.jit().counters;
    assert!(c.tiered_up > 0, "nothing moved");
    assert_eq!(c.retiered, 1, "the trace was not compiled again once");
    let (relinked, moved) = root(f);
    assert!(moved, "the loop's trace did not move");
    assert!(
        relinked > 10_000,
        "{relinked} passes went on through the side trace"
    );
}

fn shared(engine: &Engine) -> (Vm, Gc<LuaClosure>) {
    let mut vm = engine.new_vm(LuaVersion::Lua54);
    vm.set_trace_hot_threshold(8);
    vm.set_call_hot_threshold(8);
    setup(vm, true)
}

/// A Vm that took the baseline code up and filled its side cell before the
/// first Vm moved the trace with its own cell empty compiles its own code
/// when it moves, which calls the side trace; a Vm taking the trace up
/// after the move runs the moved code and compiles again once its cell is
/// filled.
#[test]
fn a_vm_whose_cells_differ_from_the_shared_code_compiles_its_own() {
    let engine = Engine::new();
    let (mut it, g) = setup(luna_jit::new_with_jit(LuaVersion::Lua54), false);
    let never = 1 << 40;
    let (mut a, fa) = shared(&engine);
    same((&mut a, fa), (&mut it, g), 500, never);
    let (mut b, fb) = shared(&engine);
    same((&mut b, fb), (&mut it, g), 500, 100);
    assert!(
        b.trace_adopted_count() > 0,
        "the second Vm compiled its own trace"
    );
    assert_eq!(b.trace_tiered_up_count(), 0, "the second Vm moved early");
    same((&mut a, fa), (&mut it, g), 5_000, never);
    assert!(a.trace_tiered_up_count() > 0, "the first Vm did not move");
    let (before, dispatched) = (root(fb).0, b.trace_dispatched_count());
    same((&mut b, fb), (&mut it, g), 5_000, 100);
    let n = b.trace_dispatched_count() - dispatched;
    assert!(
        n < 100,
        "{n} dispatches: the second Vm's code left the side trace to the dispatcher"
    );
    let (after, moved) = root(fb);
    assert!(moved, "the second Vm did not move");
    assert!(
        after - before > 4_000,
        "the second Vm's moved code left {} passes to the dispatcher",
        4_900 - (after - before)
    );
    let (mut c, fc) = shared(&engine);
    same((&mut c, fc), (&mut it, g), 20_000, 5_000);
    assert!(
        c.trace_adopted_count() > 0,
        "the third Vm compiled its own trace"
    );
    assert_eq!(
        c.jit().counters.retiered,
        1,
        "the third Vm did not compile again"
    );
    assert!(
        root(fc).0 > 10_000,
        "the third Vm's code went on in no loop"
    );
}
