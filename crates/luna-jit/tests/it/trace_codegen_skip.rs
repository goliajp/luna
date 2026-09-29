//! A trace nothing can enter (not dispatchable, no down-recursion link,
//! not a side trace a parent may call) is cached without running
//! Cranelift: the cache entry keeps its head from being recorded again,
//! and machine code would never run.

use luna_jit::jit_backend::trace::trace_codegen_count;
use luna_jit::runtime::{Gc, LuaClosure, Value};
use luna_jit::version::LuaVersion;

fn enterable(cl: Gc<LuaClosure>) -> (usize, usize) {
    let traces = cl.proto.traces.borrow();
    let n = traces
        .iter()
        .filter(|t| t.dispatchable || t.downrec_link.is_some())
        .count();
    (traces.len(), n)
}

#[test]
fn undispatchable_trace_is_cached_without_codegen() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    let before = trace_codegen_count();
    let main = vm
        .load(
            b"
            local o = {t = {}}
            local function get(self, k) return self.t[k] end
            local n = 0
            for i = 1, 500 do if get(o, i) == nil then n = n + 1 end end
            return n, get
        ",
            b"=skip",
        )
        .expect("load");
    let r = vm.call_value(Value::Closure(main), &[]).expect("run");
    assert!(matches!(r[0], Value::Int(500)), "{r:?}");
    let Value::Closure(get) = r[1] else { panic!() };
    let codegen = trace_codegen_count() - before;

    let (get_cached, get_enterable) = enterable(get);
    let (main_cached, main_enterable) = enterable(main);
    assert!(
        get_cached > 0 && get_enterable == 0,
        "get: {get_cached} cached, {get_enterable} enterable"
    );
    assert_eq!(vm.trace_compiled_count(), (get_cached + main_cached) as u64);
    // no side trace in this program: code exactly for what can be entered
    assert_eq!(vm.trace_side_trace_compiled_count(), 0);
    assert_eq!(codegen, (get_enterable + main_enterable) as u64);
    // the cached entry stops `get` from being recorded again
    let closed = vm.trace_closed_count();
    vm.call_value(Value::Closure(main), &[]).expect("rerun");
    assert_eq!(vm.trace_closed_count(), closed);
}

#[test]
fn downrec_linked_trace_still_gets_code() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_self_link_enabled(true);
    vm.open_base();
    let before = trace_codegen_count();
    let main = vm
        .load(
            b"
            local function fib(n)
                if n < 2 then return n end
                return fib(n - 1) + fib(n - 2)
            end
            local s = 0
            for i = 1, 200 do s = s + fib(3) end
            return s, fib
        ",
            b"=fib3",
        )
        .expect("load");
    let r = vm.call_value(Value::Closure(main), &[]).expect("run");
    assert!(matches!(r[0], Value::Int(400)), "{r:?}");
    let Value::Closure(fib) = r[1] else { panic!() };
    let (_, fib_enterable) = enterable(fib);
    let (_, main_enterable) = enterable(main);
    assert!(fib_enterable > 0);
    // side traces are pinned undispatchable after their code is made,
    // so they only add to the left side
    assert!(trace_codegen_count() - before >= (fib_enterable + main_enterable) as u64);
    assert!(vm.trace_downrec_dispatched_count() + vm.trace_downrec_deopt_count() > 0);
}
