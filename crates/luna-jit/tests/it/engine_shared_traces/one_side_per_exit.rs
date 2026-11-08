//! An exit of a shared trace runs one side trace, however many the engine
//! holds for it.

use super::*;

/// A loop whose branch past `m` reads `k`, which the loop before it never
/// does: the side trace there checks `k`'s tag itself, so a Vm whose `k` is
/// a table compiles one of its own beside the one a Vm with a string made.
const BRANCH: &str = r#"
return function(k, n, m)
    local s = 0
    for i = 1, n do
        if i > m then s = s + #k else s = s + 1 end
    end
    return s
end
"#;

/// [`BRANCH`]'s function, loaded in `vm`.
fn load(vm: &mut Vm) -> Value {
    let main = vm.load(BRANCH.as_bytes(), b"=branch").expect("load");
    vm.call_value(Value::Closure(main), &[]).expect("chunk")[0]
}

/// `f(k, n, m)` in `vm`, `k` a string unless `table`.
fn call(vm: &mut Vm, f: Value, table: bool, (n, m): (u32, u32)) -> String {
    let k = if table { "{1, 2, 3}" } else { "'abc'" };
    let src = format!("local f = ... return f({k}, {n}, {m})");
    let wrap = vm.load(src.as_bytes(), b"=call").expect("load");
    match vm.call_value(Value::Closure(wrap), &[f]) {
        Ok(v) => v.iter().map(show).collect::<Vec<_>>().join(", "),
        Err(e) => format!("error: {}", vm.error_display(&e)),
    }
}

#[test]
fn a_trace_taken_up_brings_one_side_trace_for_each_exit() {
    let (straight, branching) = ((200, 200), (200, 100));
    for table in [false, true] {
        let mut it = interp(LuaVersion::Lua54);
        let f = load(&mut it);
        let want = [straight, branching].map(|nm| call(&mut it, f, table, nm));
        let engine = Engine::new();
        // the loop's trace, compiled where the branch is never taken
        let mut a = shared(&engine, LuaVersion::Lua54);
        let fa = load(&mut a);
        call(&mut a, fa, false, straight);
        // two Vms take it up before either has a side trace for the branch,
        // then each compiles one: for a string `k`, and for a table
        let (mut b, mut c) = (
            shared(&engine, LuaVersion::Lua54),
            shared(&engine, LuaVersion::Lua54),
        );
        let (fb, fc) = (load(&mut b), load(&mut c));
        call(&mut b, fb, false, straight);
        call(&mut c, fc, true, straight);
        assert!(b.trace_adopted_count() > 0 && c.trace_adopted_count() > 0);
        let before = b.trace_compiled_count() + c.trace_compiled_count();
        call(&mut b, fb, false, branching);
        call(&mut c, fc, true, branching);
        assert_eq!(
            b.trace_compiled_count() + c.trace_compiled_count() - before,
            2,
            "each Vm compiles a side trace of its own"
        );
        // a fourth Vm takes up the loop's trace with the first cached of
        // them only
        let mut d = shared(&engine, LuaVersion::Lua54);
        let fd = load(&mut d);
        let got = [straight, branching].map(|nm| call(&mut d, fd, table, nm));
        assert_eq!(got, want, "table {table}");
        assert_eq!(d.trace_adopted_count(), 2, "table {table}");
    }
}
