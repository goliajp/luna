//! `#t` answers from the table's array-part counters when the array holds
//! exactly a leading run. The JIT's inline array stores bypass
//! `Table::set`, so they must keep those counters too: JIT on and JIT off
//! give the same lengths.

use luna_jit::runtime::Value;
use luna_jit::runtime::function::JitProtoState;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

// each returns a length the counters decide
const PROGS: &[(&str, i64)] = &[
    // the whole chunk runs in the method JIT: `t[i] = i` stores inline
    // into the nil slots each array doubling leaves
    (
        "local t = {} for i = 1, 10000 do t[i] = i end return #t",
        10000,
    ),
    // constructor literals (SetList) in a method-JIT'd function
    (
        "local function mk() return {1, 2, 3, 4, 5} end \
         local s = 0 for i = 1, 300 do s = s + #mk() end return s",
        1500,
    ),
    // appends at `#t + 1`, trace JIT
    (
        "local t = {} for i = 1, 3000 do t[#t + 1] = i end return #t",
        3000,
    ),
    // a hole that is refilled
    (
        "local t = {} for i = 1, 100 do t[i] = i end \
         for i = 50, 100 do t[i] = nil end \
         for i = 50, 60 do t[i] = i end return #t",
        60,
    ),
];

fn int(vm: &mut Vm, src: &str) -> i64 {
    match vm.eval(src).expect("eval")[0] {
        Value::Int(i) => i,
        v => panic!("expected an integer, got {v:?}"),
    }
}

#[test]
fn lengths_agree_with_the_jit_on_and_off() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for &(src, want) in PROGS {
            let mut off = Vm::new(v);
            assert_eq!(int(&mut off, src), want, "{v:?} jit off: {src}");
            let mut on = luna_jit::new_with_jit(v);
            assert_eq!(int(&mut on, src), want, "{v:?} jit on: {src}");
        }
    }
}

#[test]
fn the_fill_loop_really_runs_in_the_method_jit() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let cl = vm.load(PROGS[0].0.as_bytes(), b"=t").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
    assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
    assert!(matches!(r[0], Value::Int(10000)));

    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let outer = vm.load(PROGS[1].0.as_bytes(), b"=t").expect("load");
    vm.call_value(Value::Closure(outer), &[]).expect("run");
    let mk = outer.proto.protos[0];
    assert!(matches!(mk.jit.get(), JitProtoState::Compiled { .. }));
}
