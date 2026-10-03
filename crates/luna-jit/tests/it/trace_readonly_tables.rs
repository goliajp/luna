//! Compiled table stores honour read-only tables (`Vm::set_readonly`). The
//! loops below store into a list of tables; their traces are compiled while
//! every table is writable, then one table in the middle of the list is
//! marked read-only. The trace reaches it while running: its inline array
//! store, its inline hash-slot store and its store helpers must all leave
//! the write to the interpreter, which raises with the position of the
//! store and leaves the table as it was. Unmarking the table lets the same
//! traces write to it again.
//!
//! A trace that stores into one table through a register nothing in the
//! trace writes tests that table once, before its loop head, instead of at
//! each store: marking the table between two runs of the trace must make
//! the next run leave at its head. A native iterator that marks the table
//! while the trace runs it must make the next store raise.

use luna_jit::jit::trace::TraceTier;
use luna_jit::jit_backend::trace::baseline_codegen_count;
use luna_jit::runtime::{Gc, Table, Value};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

const MSG: &str = "Attempt to modify a readonly table";

/// One store per function, each on its own line (the error's line):
/// `A` an integer key in the array part, `F` a number into the hash slot
/// the key was recorded in, `H` a table value through the store helper,
/// `K` a string key in a register, `I` an integer key in a register, `N`
/// new keys past the array part. `G` stores a global; the trace JIT does
/// not compile its loop, so it checks the interpreter's path. `V` and `VA`
/// store into one table every iteration (a hash slot and an array slot);
/// `R` stores while a native iterator runs, which marks `MARK` on its 150th
/// call. `VC(t, 1)` stores nothing in the first iteration, which the
/// interpreter runs; the trace entered at the second must test the table
/// before its first store.
const SRC: &str = "\
function A(ts) local c = 0 for _, t in ipairs(ts) do c = c + 1 t[1] = c end end
function F(ts) local c = 0 for _, t in ipairs(ts) do c = c + 1 t.x = c end end
function H(ts) for _, t in ipairs(ts) do t.s = ts end end
function K(ts) local k, c = 'x', 0 for _, t in ipairs(ts) do c = c + 1 t[k] = c end end
function I(ts) local j, c = 2, 0 for _, t in ipairs(ts) do c = c + 1 t[j] = c end end
function N(ts) local c = 0 for _, t in ipairs(ts) do c = c + 1 t[c + 10] = c end end
function G(n) for i = 1, n do GV = i end end
function V(t) local i = 0 while i < 300 do i = i + 1 t.x = i end end
function VA(t) local i = 0 while i < 300 do i = i + 1 t[1] = i end end
function R(t) local c = 0 for k, v in ITER, 0, 0 do c = c + 1 t.x = c end end
function VC(t, n) local i = 0 while i < 300 do i = i + 1 if i > n then t.x = i end end end
function fresh(n)
  local ts = {}
  for i = 1, n do ts[i] = {0, 0, x = 0.5, s = false} end
  return ts
end
TS = fresh(60)
";

const STORES: [(&str, u32); 6] = [("A", 1), ("F", 2), ("H", 3), ("K", 4), ("I", 5), ("N", 6)];

/// The 40th table is as `fresh` made it.
const UNTOUCHED: &str = "local t = TS[40] local n = 0 for _ in pairs(t) do n = n + 1 end \
    return t[1] == 0 and t[2] == 0 and t.x == 0.5 and t.s == false and n == 4";

/// `ITER(_, k)`: `k + 1` twice, up to 300; on its 150th call it marks the
/// table in the global `MARK` read-only, as a host function may.
fn iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_jit::vm::LuaError> {
    let k = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) => i,
        Value::Float(f) => f as i64,
        _ => 0,
    } + 1;
    if k > 300 {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    if k == 150
        && let Value::Table(t) = vm.globals().get(Value::Str(vm.intern_str("MARK")))
    {
        vm.set_readonly(t, true);
    }
    let v = if vm.version() <= LuaVersion::Lua52 {
        Value::Float(k as f64)
    } else {
        Value::Int(k)
    };
    Ok(vm.nat_return(fs, &[v, v]))
}

fn call(vm: &mut Vm, f: &str, args: &[Value]) -> Result<(), String> {
    let fv = vm.globals().get(Value::Str(vm.intern_str(f)));
    vm.call_value(fv, args)
        .map(|_| ())
        .map_err(|e| vm.error_text(&e))
}

fn check(vm: &mut Vm, src: &str, what: &str) {
    match vm.eval(src) {
        Ok(r) => assert!(
            matches!(r[..], [Value::Bool(true)]),
            "{what}: {src} gave {r:?}"
        ),
        Err(e) => panic!("{what}: {src}: {}", vm.error_text(&e)),
    }
}

fn table(vm: &mut Vm, expr: &str) -> Gc<Table> {
    match vm.eval(&format!("return {expr}")).expect("expr")[0] {
        Value::Table(t) => t,
        ref v => panic!("{expr} is {v:?}"),
    }
}

/// A Vm with traces recorded after one back edge, in `tier`; `method` also
/// turns the method JIT on.
fn vm(v: LuaVersion, tier: TraceTier, method: bool) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(method);
    vm.set_trace_jit_enabled(true);
    vm.jit.trace_hot_threshold = 1;
    vm.jit.call_hot_threshold = 1;
    vm.set_trace_tier(tier);
    let f = vm.native(iter);
    vm.set_global("ITER", f).expect("ITER");
    let main = vm.load(SRC.as_bytes(), b"=user_script").expect("load");
    vm.call_value(Value::Closure(main), &[]).expect("chunk");
    vm
}

fn run(v: LuaVersion, tier: TraceTier, method: bool) {
    let what = format!("{v:?} {tier:?} method JIT {method}");
    let mut vm = vm(v, tier, method);
    let ts = Value::Table(table(&mut vm, "TS"));
    let n = Value::Int(60);
    for (f, _) in STORES {
        for _ in 0..3 {
            call(&mut vm, f, &[ts]).unwrap_or_else(|e| panic!("{what}: warm {f}: {e}"));
        }
    }
    for _ in 0..3 {
        call(&mut vm, "G", &[n]).unwrap_or_else(|e| panic!("{what}: warm G: {e}"));
    }
    let warm = vm.trace_dispatched_count();
    assert!(warm > 0, "{what}: no trace ran while warming up");
    vm.eval("TS = fresh(60)").expect("refill");
    let ts = Value::Table(table(&mut vm, "TS"));
    let ro = table(&mut vm, "TS[40]");
    vm.set_readonly(ro, true);
    for (f, line) in STORES {
        let before = vm.trace_dispatched_count();
        let e = call(&mut vm, f, &[ts]).expect_err(&format!("{what}: {f} wrote"));
        assert_eq!(e, format!("user_script:{line}: {MSG}"), "{what}: {f}");
        assert!(
            vm.trace_dispatched_count() > before,
            "{what}: {f} ran no trace ({before} entries before)"
        );
        check(&mut vm, UNTOUCHED, &format!("{what}: {f}"));
    }
    // the tables before the read-only one were written by the traces
    check(
        &mut vm,
        "local t = TS[39] return t[1] == 39 and t.x == 39 and t[2] == 39 \
         and t.s == TS and t[49] == 39",
        &what,
    );
    let g = vm.globals();
    vm.set_readonly(g, true);
    let e = call(&mut vm, "G", &[n]).expect_err(&format!("{what}: G wrote"));
    assert_eq!(e, format!("user_script:7: {MSG}"), "{what}: G");
    vm.set_readonly(g, false);

    // writable again: the same traces store into it
    vm.set_readonly(ro, false);
    let before = vm.trace_dispatched_count();
    for (f, _) in STORES {
        call(&mut vm, f, &[ts]).unwrap_or_else(|e| panic!("{what}: {f} after unmarking: {e}"));
    }
    call(&mut vm, "G", &[n]).unwrap_or_else(|e| panic!("{what}: G after unmarking: {e}"));
    assert!(
        vm.trace_dispatched_count() > before,
        "{what}: no trace after unmarking"
    );
    check(
        &mut vm,
        "local t = TS[40] return t[1] == 40 and t.x == 40 and t[2] == 40 \
         and t.s == TS and t[50] == 40 and GV == 60",
        &what,
    );
}

/// A number as dialect `v` holds it.
fn num(v: LuaVersion, n: i64) -> Value {
    if v <= LuaVersion::Lua52 {
        Value::Float(n as f64)
    } else {
        Value::Int(n)
    }
}

/// `V`, `VA`, `R` and `VC` (see [`SRC`]) under `tier`.
fn run_one_table(v: LuaVersion, tier: TraceTier, method: bool) {
    let what = format!("{v:?} {tier:?} method JIT {method}");
    let mut vm = vm(v, tier, method);
    vm.eval("T = {1, x = 0} MARK = {}").expect("setup");
    let t = table(&mut vm, "T");
    let tv = Value::Table(t);
    for f in ["V", "VA", "R", "VC"] {
        for _ in 0..3 {
            call(&mut vm, f, &[tv, num(v, 0)]).unwrap_or_else(|e| panic!("{what}: warm {f}: {e}"));
        }
    }
    assert!(vm.trace_dispatched_count() > 0, "{what}: no trace ran");
    // marked between runs: the trace leaves at its head
    vm.set_readonly(t, true);
    for (f, line) in [("V", 8), ("VA", 9)] {
        let e = call(&mut vm, f, &[tv]).expect_err(&format!("{what}: {f} wrote"));
        assert_eq!(e, format!("user_script:{line}: {MSG}"), "{what}: {f}");
        check(
            &mut vm,
            "return T.x == 300 and T[1] == 300",
            &format!("{what}: {f}"),
        );
    }
    vm.set_readonly(t, false);
    let before = vm.trace_dispatched_count();
    call(&mut vm, "V", &[tv]).unwrap_or_else(|e| panic!("{what}: V after unmarking: {e}"));
    assert!(
        vm.trace_dispatched_count() > before,
        "{what}: V ran no trace"
    );
    // marked while the trace runs the iterator: the next store raises
    vm.eval("T.x = 0 MARK = T").expect("arm");
    let before = vm.trace_dispatched_count();
    let e = call(&mut vm, "R", &[tv]).expect_err(&format!("{what}: R wrote"));
    assert_eq!(e, format!("user_script:10: {MSG}"), "{what}: R");
    assert!(
        vm.trace_dispatched_count() > before,
        "{what}: R ran no trace"
    );
    check(&mut vm, "return T.x == 149", &format!("{what}: R"));
    // the trace's first store comes before any store of the interpreter's
    vm.set_readonly(t, false);
    vm.eval("T.x = 0").expect("reset");
    vm.set_readonly(t, true);
    let before = vm.trace_dispatched_count();
    let e = call(&mut vm, "VC", &[tv, num(v, 1)]).expect_err(&format!("{what}: VC wrote"));
    assert_eq!(e, format!("user_script:11: {MSG}"), "{what}: VC");
    assert!(
        vm.trace_dispatched_count() > before,
        "{what}: VC ran no trace"
    );
    check(&mut vm, "return T.x == 0", &format!("{what}: VC"));
}

#[test]
fn a_table_tested_once_per_trace_run_is_tested_again_after_host_code() {
    for v in DIALECTS {
        for tier in [TraceTier::Baseline, TraceTier::Optimizing] {
            run_one_table(v, tier, false);
        }
        run_one_table(v, TraceTier::Auto, true);
    }
}

/// The method JIT stores into the tables its function is given without
/// testing them; the call tests each table argument before it runs the
/// compiled code, and hands a read-only one to the interpreter, which
/// raises.
#[test]
fn the_method_jit_hands_a_read_only_table_argument_to_the_interpreter() {
    use luna_jit::runtime::function::JitProtoState;
    let src = "\
function W(t, n) for i = 1, n do t[i] = i end return n end
function M(n) local t = {} for i = 1, n do t[i] = i end return n end
T = {0, 0, 0}
";
    for v in DIALECTS {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(true);
        vm.set_trace_jit_enabled(false);
        vm.jit.call_hot_threshold = 1;
        let main = vm.load(src.as_bytes(), b"=user_script").expect("load");
        vm.call_value(Value::Closure(main), &[]).expect("chunk");
        let t = Value::Table(table(&mut vm, "T"));
        for _ in 0..20 {
            call(&mut vm, "W", &[t, Value::Int(3)]).expect("W");
            call(&mut vm, "M", &[Value::Int(3)]).expect("M");
        }
        let proto = |vm: &mut Vm, f: &str| match vm.globals().get(Value::Str(vm.intern_str(f))) {
            Value::Closure(c) => c.proto,
            v => panic!("{f} is {v:?}"),
        };
        // the method JIT takes integer arguments, which 5.1 and 5.2 lack
        if v >= LuaVersion::Lua53 {
            assert!(
                matches!(
                    proto(&mut vm, "M").jit.get(),
                    JitProtoState::Compiled { .. }
                ),
                "{v:?}: the method JIT did not compile M"
            );
        }
        if v >= LuaVersion::Lua53 {
            assert!(
                matches!(
                    proto(&mut vm, "W").jit.get(),
                    JitProtoState::Compiled { .. }
                ),
                "{v:?}: the method JIT did not compile W"
            );
        }
        let Value::Table(tt) = t else { unreachable!() };
        vm.set_readonly(tt, true);
        let e = call(&mut vm, "W", &[t, Value::Int(3)]).expect_err("W wrote");
        assert_eq!(e, format!("user_script:1: {MSG}"), "{v:?}");
        check(
            &mut vm,
            "return T[1] == 1 and T[2] == 2 and T[3] == 3",
            &format!("{v:?}"),
        );
        vm.eval("T[1] = 0").expect_err("T is read-only");
        vm.set_readonly(tt, false);
        call(&mut vm, "W", &[t, Value::Int(3)]).expect("W after unmarking");
    }
}

#[test]
fn baseline_traces_refuse_a_table_marked_after_they_were_compiled() {
    let before = baseline_codegen_count();
    for v in DIALECTS {
        run(v, TraceTier::Baseline, false);
    }
    assert!(
        baseline_codegen_count() > before,
        "no baseline code was made"
    );
}

#[test]
fn cranelift_traces_refuse_a_table_marked_after_they_were_compiled() {
    let before = baseline_codegen_count();
    for v in DIALECTS {
        run(v, TraceTier::Optimizing, false);
    }
    assert_eq!(
        baseline_codegen_count(),
        before,
        "Optimizing made baseline code"
    );
}

#[test]
fn the_default_jit_setup_refuses_it_too() {
    for v in DIALECTS {
        run(v, TraceTier::Auto, true);
    }
}
