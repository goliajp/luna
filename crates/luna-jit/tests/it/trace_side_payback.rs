//! A side trace moves to the optimizing tier once the move has paid for
//! itself, whether or not its function is called again, and sooner with a
//! lower `tier_up_at`, as a root trace does.

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

/// Past its first 100 passes the loop leaves each pass for the side trace
/// of the first way of its body: `n - 100` passes of the side trace a call.
const SRC: &str = "
    return function(n)
      local s = 0
      for i = 1, n do
        if i > 100 then s = s + 1 else s = s + 2 end
      end
      return s
    end";

/// The function the chunk returns, in a Vm with the trace JIT on or off,
/// its traces moving at `at`.
fn function(v: LuaVersion, jit: bool, at: Option<u32>) -> (Vm, Gc<LuaClosure>) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    if let Some(at) = at {
        vm.set_trace_tier_up_at(at);
    }
    let main = vm.load(SRC.as_bytes(), b"=payback").expect("load");
    let Value::Closure(f) = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] else {
        panic!("no function");
    };
    (vm, f)
}

fn call(vm: &mut Vm, f: Gc<LuaClosure>, n: i64) -> String {
    let n = match vm.version() {
        LuaVersion::Lua51 | LuaVersion::Lua52 => Value::Float(n as f64),
        _ => Value::Int(n),
    };
    format!(
        "{:?}",
        vm.call_value(Value::Closure(f), &[n]).expect("call")
    )
}

/// Whether the loop's side trace is held, and runs the optimizing tier's
/// code.
fn side(f: Gc<LuaClosure>) -> (bool, bool) {
    let traces = f.proto.traces();
    let root = (traces.iter())
        .find(|t| !t.side_cell_states().is_empty())
        .expect("a trace with side cells");
    let states = root.side_cell_states();
    (states.iter().any(|s| s.0), states.iter().any(|s| s.1))
}

/// 30000 passes, past the root traces' 16384 and their quarter once the
/// function is called again, are short of any side trace's move; ten
/// calls of them are past it.
#[test]
fn a_side_trace_moves_once_the_move_pays_for_itself() {
    for v in VERSIONS {
        let (mut it, g) = function(v, false, None);
        let (mut vm, f) = function(v, true, None);
        assert_eq!(call(&mut vm, f, 30_100), call(&mut it, g, 30_100), "{v:?}");
        assert_eq!(side(f), (true, false), "{v:?}: moved in the first call");
        for _ in 1..10 {
            assert_eq!(call(&mut vm, f, 30_100), call(&mut it, g, 30_100), "{v:?}");
        }
        assert_eq!(side(f), (true, true), "{v:?}: not moved");
    }
}

#[test]
fn a_lower_tier_up_at_moves_a_side_trace_sooner() {
    for v in VERSIONS {
        let (mut it, g) = function(v, false, Some(50));
        let (mut vm, f) = function(v, true, Some(50));
        assert_eq!(call(&mut vm, f, 2_100), call(&mut it, g, 2_100), "{v:?}");
        assert_eq!(side(f), (true, true), "{v:?}: not moved");
    }
}
