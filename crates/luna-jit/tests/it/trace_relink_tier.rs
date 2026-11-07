//! A side trace going on in its parent's loop when the loop's count reaches
//! the move to the optimizing tier, with registers of other kinds than the
//! optimizing tier's loop is compiled for: the parent's code leaves at the
//! loop's head, and the dispatcher moves the trace and enters the
//! optimizing tier's entry.

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

const AT: u32 = 1000;

/// With `LIM` 100 the trace takes the second way of the loop's body, the
/// first 100 passes' way; past them every pass leaves for the side trace,
/// which ends at the head without writing `t`. The trace's loop writes `t` before it reads
/// it, so `t` is not checked on entry, and the side trace's registers may
/// not go on in the optimizing tier's loop, which takes `t` as the back edge
/// leaves it. The loop's count reaches the move in the relink block.
const SRC: &str = "
    return function()
      local s = 0
      for i = 1, 20000 do
        if i > LIM then s = s + 1 else local t = i * 2; s = s + t end
      end
      return s
    end";

/// The function the chunk returns, in a Vm with the trace JIT on or off.
fn function(v: LuaVersion, jit: bool) -> (Vm, Gc<LuaClosure>) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_tier_up_at(AT);
    vm.set_global("LIM", number(v, 100)).expect("LIM");
    let main = vm.load(SRC.as_bytes(), b"=tier").expect("load");
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

fn call(vm: &mut Vm, f: Gc<LuaClosure>) -> String {
    format!("{:?}", vm.call_value(Value::Closure(f), &[]).expect("call"))
}

/// The loop's trace (the one holding side cells): its count towards the
/// move, the side traces it went on with in its loop, and whether the
/// dispatcher enters its optimizing tier's code.
fn root_state(f: Gc<LuaClosure>) -> (u32, u64, bool) {
    let traces = f.proto.traces();
    let root = (traces.iter())
        .find(|t| !t.side_cell_states().is_empty())
        .expect("a trace with side cells");
    let count = root.tier_up().expect("a trace that moves").count().get();
    let moved = root.current_entry() as usize != root.entry() as usize;
    (count, root.side_relinks(), moved)
}

/// The call that reaches the count leaves for the dispatcher, which moves
/// the trace within the call; the next call runs in the optimizing tier's
/// code, whose side trace, of the same kinds, goes on in its loop there.
/// With every pass leaving for the side trace, the first one too, the
/// optimizing tier's first pass goes on in its own code rather than leaving
/// at each pass.
#[test]
fn a_side_trace_reaching_the_move_leaves_for_the_optimizing_entry() {
    for v in VERSIONS {
        let (mut it, g) = function(v, false);
        let (mut vm, f) = function(v, true);
        assert_eq!(call(&mut vm, f), call(&mut it, g), "{v:?}");
        let (count, relinked, moved) = root_state(f);
        assert!(relinked > 0, "{v:?}: no side trace went on in the loop");
        assert!(moved, "{v:?}: the loop's trace did not move");
        assert!(
            count <= AT + 1,
            "{v:?}: the baseline code ran on past the move, count {count}"
        );
        let tiered = vm.trace_tiered_up_count();
        assert_eq!(call(&mut vm, f), call(&mut it, g), "{v:?}");
        // the optimizing tier's first pass sets the count back by one as it
        // hands over to its loop, and nothing counts after
        let (after, relinked_after, _) = root_state(f);
        assert_eq!(after, AT - 1, "{v:?}: the next call ran the baseline code");
        assert!(
            relinked_after > relinked,
            "{v:?}: the optimizing tier's code went on in no loop"
        );
        assert_eq!(vm.trace_tiered_up_count(), tiered, "{v:?}");
        for vm in [&mut vm, &mut it] {
            vm.set_global("LIM", number(v, 0)).expect("LIM");
        }
        let dispatched = vm.trace_dispatched_count();
        assert_eq!(call(&mut vm, f), call(&mut it, g), "{v:?}");
        let n = vm.trace_dispatched_count() - dispatched;
        assert!(n < 10, "{v:?}: {n} dispatches for one call");
        assert!(root_state(f).1 > relinked_after + 10_000, "{v:?}");
    }
}
