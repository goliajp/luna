//! 5.2's step into the sweep: what the atomic step and `entersweep`
//! count.

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
