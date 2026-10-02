//! A trace the lowerer marked not dispatchable (it holds a value of a
//! type it could not work out) stays that way when it closes as a
//! recursive trace. The multi-way DownRec close used to set
//! `dispatchable = true` over that mark, and the single-way close left a
//! `downrec_link` that the dispatcher admits a trace on by itself.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

#[test]
fn downrec_close_keeps_an_earlier_dispatch_off_mark() {
    // `t` read through an upvalue and not called: a boolean is not a type
    // the lowerer reads an upvalue as, so it marks the trace not
    // dispatchable
    let src = b"
        local t = true
        local function fib(n)
            local v = t
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        local s = 0
        for i = 1, 200 do s = s + fib(3) end
        return fib, s
    ";
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();
    let cl = vm.load(src, b"=fib_upval").expect("loads");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("runs");
    assert!(matches!(r.get(1), Some(Value::Int(400))), "got {r:?}");
    let Some(Value::Closure(fib)) = r.first() else {
        panic!("fib not returned: {r:?}")
    };

    // every trace of `fib` reads `t`, so none may run
    let traces = fib.proto.traces.borrow();
    assert!(!traces.is_empty(), "fib compiled no trace");
    for ct in traces.iter() {
        assert!(
            !ct.dispatchable,
            "trace at pc {} is dispatchable",
            ct.head_pc
        );
        assert!(
            ct.downrec_link.is_none(),
            "trace at pc {} can be admitted as a DownRec trace",
            ct.head_pc
        );
    }
    assert!(
        traces
            .iter()
            .any(|ct| ct.dispatch_off_reason == Some("GetUpval:not-Closure-use")),
        "no trace was marked for the upvalue read"
    );
}
