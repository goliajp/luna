//! The step into the sweep: what the atomic step and `entersweep` count
//! (5.2), and what 5.3's leaves out.

use super::*;
use crate::version::LuaVersion;
use crate::vm::Vm;

/// A 5.2 state whose marking is over, the gray list empty: the atomic
/// step is next.
fn marked() -> Vm {
    let mut vm = Vm::new_minimal_with_hash_seed(LuaVersion::Lua52, 1);
    vm.open_all_libs().expect("the libraries open");
    vm.eval(
        "collectgarbage('stop') collectgarbage()
         keep = {}
         for i = 1, 40 do keep[i] = {i}; setmetatable({}, {__gc = function() end}) end
         for i = 1, 40 do local _ = {i} end",
    )
    .expect("the script runs");
    assert!(vm.heap.puc.pace.state() == St::Pause);
    singlestep_52(&mut vm);
    let run = vm.running();
    let mut work = i64::MAX;
    while !vm.propagate(&mut work, i64::MIN, run) {}
    vm
}

#[test]
fn entering_the_sweep_counts_finobj_s_sweep_too() {
    // the same state twice: the atomic step's work and what the sweep
    // into allgc went over, then the step that enters the sweep
    let (work, swept) = atomic(&mut marked());
    let mut vm = marked();
    let cost = sweepcost_52(&vm.heap.puc.sizes);
    // `entersweep`: `sweeptolive` on `finobj` goes over one object
    assert_eq!(atomic_52(&mut vm), work + (swept + 1) * cost);
}

/// A 5.3 state whose marking is over (its step into the atomic phase is
/// next) and that then ran `after`, the collector stopped.
fn marked_53(after: &str) -> Vm {
    let mut vm = Vm::new_minimal_with_hash_seed(LuaVersion::Lua53, 1);
    vm.open_all_libs().expect("the libraries open");
    vm.eval("collectgarbage('stop') collectgarbage() keep = {}")
        .expect("the script runs");
    assert!(vm.heap.puc.pace.state() == St::Pause);
    singlestep_53(&mut vm).expect("the cycle starts");
    let run = vm.running();
    let mut work = i64::MAX;
    while !vm.propagate(&mut work, i64::MIN, run) {}
    vm.heap.puc.pace.enter(St::Atomic);
    vm.eval(after).expect("the script runs");
    vm
}

#[test]
fn the_5_3_atomic_step_counts_nothing_of_what_was_turned_gray_since_marking_ended() {
    // a black table takes a white metatable (a forward barrier turns it
    // gray), or a white one does (nothing turns gray): `singlestep`'s
    // `propagateall` traverses the gray one before `atomic` counts
    let mt = "{1, 2, 3, 4, 5, 6, 7, 8, a = 1, b = 2, c = 3, d = 4}";
    let gray = singlestep_53(&mut marked_53(&format!("debug.setmetatable(keep, {mt})")));
    let none = singlestep_53(&mut marked_53(&format!(
        "local x = {{}} debug.setmetatable(x, {mt})"
    )));
    assert_eq!(gray.expect("the step runs"), none.expect("the step runs"));
}

/// A state whose marking has reached everything (`keep` included) and is
/// still in its propagate phase, then dropped `keep`, the collector
/// stopped; `weak` holds `keep` weakly.
fn marking_then_dropped(version: LuaVersion, start: fn(&mut Vm)) -> Vm {
    let mut vm = Vm::new_minimal_with_hash_seed(version, 1);
    vm.open_all_libs().expect("the libraries open");
    vm.eval(
        "collectgarbage('stop') collectgarbage()
         keep = {} weak = setmetatable({keep}, {__mode = 'v'})",
    )
    .expect("the script runs");
    start(&mut vm);
    let run = vm.running();
    let mut work = i64::MAX;
    while !vm.propagate(&mut work, i64::MIN, run) {}
    vm.eval("keep = nil").expect("the script runs");
    assert!(vm.heap.gc_phase_is_propagate());
    vm
}

#[test]
fn an_emergency_while_marking_starts_the_collection_over() {
    // `luaC_fullgc` drops the marking (`entersweep`) and runs a whole
    // cycle: what was marked and then dropped is freed
    let starts: [(LuaVersion, fn(&mut Vm)); 4] = [
        (LuaVersion::Lua52, |vm| {
            // 5.2 runs none while its collector is stopped
            vm.heap.puc.stopped = false;
            singlestep_52(vm);
        }),
        (LuaVersion::Lua53, |vm| {
            singlestep_53(vm).expect("the cycle starts");
        }),
        (LuaVersion::Lua54, |vm| {
            singlestep_54(vm).expect("the cycle starts");
        }),
        (LuaVersion::Lua55, |vm| {
            singlestep_55(vm, false).expect("the cycle starts");
        }),
    ];
    for (version, start) in starts {
        let mut vm = marking_then_dropped(version, start);
        assert!(vm.emergency_collect());
        let left = vm.eval("return weak[1] == nil").expect("the script runs");
        assert!(
            matches!(left[0], crate::runtime::Value::Bool(true)),
            "{version:?}"
        );
    }
}
