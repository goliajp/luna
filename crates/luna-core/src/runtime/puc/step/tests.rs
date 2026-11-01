//! The step into the sweep: what the atomic step and `entersweep` count
//! (5.2), and what 5.3's leaves out.

use super::*;
use crate::runtime::Value;
use crate::runtime::puc::api::full_collect;
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
    let kept: Vec<LuaVersion> = starts
        .into_iter()
        .filter(|&(version, start)| {
            let mut vm = marking_then_dropped(version, start);
            assert!(vm.emergency_collect());
            let left = vm.eval("return weak[1] == nil").expect("the script runs");
            !matches!(left[0], crate::runtime::Value::Bool(true))
        })
        .map(|(version, _)| version)
        .collect();
    assert!(kept.is_empty(), "kept in {kept:?}");
}

/// An emergency in the middle of 5.2's sweep: the threads the sweep kept are
/// dropped, not shrunk, before the emergency's cycle starts (a build with
/// `DEBUG_CHECKS` sees them left over at that cycle's restart), and the
/// cycle frees what died before the atomic step.
#[test]
fn a_5_2_emergency_while_sweeping_drops_the_kept_threads_first() {
    let mut vm = marking_then_dropped(LuaVersion::Lua52, |vm| {
        vm.heap.puc.stopped = false;
        singlestep_52(vm);
    });
    while !matches!(vm.heap.puc.pace.state(), St::SweepAll) {
        singlestep_52(&mut vm);
    }
    assert!(vm.emergency_collect());
    assert!(vm.heap.puc.records.swept_52.is_empty());
    // `luaC_fullgc` finishes the sweep and runs a whole cycle: what was
    // marked and then dropped is freed, with no safe point since the atomic
    // step as well
    let left = vm.eval("return weak[1] == nil").expect("the script runs");
    assert!(matches!(left[0], crate::runtime::Value::Bool(true)));
}

fn sweeping(s: St) -> bool {
    matches!(
        s,
        St::SweepString | St::SweepUdata | St::SweepAll | St::SweepFinobj | St::SweepTobefnz
    )
}

/// An emergency in the middle of the sweep, 5.3 to 5.5 (5.2's is above;
/// 5.1 has no emergency collection): what was marked and then dropped is
/// freed by the emergency's cycle, though no safe point came after the
/// atomic step.
#[test]
fn an_emergency_while_sweeping_frees_what_died_before_the_atomic_step() {
    let runs: [(LuaVersion, fn(&mut Vm)); 3] = [
        (LuaVersion::Lua53, |vm| {
            singlestep_53(vm).expect("a step");
        }),
        (LuaVersion::Lua54, |vm| {
            singlestep_54(vm).expect("a step");
        }),
        (LuaVersion::Lua55, |vm| {
            singlestep_55(vm, false).expect("a step");
        }),
    ];
    let kept: Vec<LuaVersion> = runs
        .into_iter()
        .filter(|&(version, step)| {
            let mut vm = marking_then_dropped(version, step);
            while !sweeping(vm.heap.puc.pace.state()) {
                step(&mut vm);
            }
            assert!(vm.emergency_collect());
            let left = vm.eval("return weak[1] == nil").expect("the script runs");
            !matches!(left[0], crate::runtime::Value::Bool(true))
        })
        .map(|(version, _)| version)
        .collect();
    assert!(kept.is_empty(), "kept in {kept:?}");
}

/// The newest object at the last safe point died before the atomic step, so
/// the young boundary moved to the object after it, `x`, which the sweep
/// keeps; `x` then dies. An emergency in the middle of the sweep frees it, as
/// PUC's `luaC_fullgc` does: it finishes the sweep and collects from the
/// roots, and nothing but a native's locals could hold an object that old.
#[test]
fn an_emergency_while_sweeping_frees_the_moved_boundary_once_it_died() {
    let runs: [(LuaVersion, fn(&mut Vm)); 4] = [
        (LuaVersion::Lua52, |vm| {
            vm.heap.puc.stopped = false;
            singlestep_52(vm);
        }),
        (LuaVersion::Lua53, |vm| {
            singlestep_53(vm).expect("a step");
        }),
        (LuaVersion::Lua54, |vm| {
            singlestep_54(vm).expect("a step");
        }),
        (LuaVersion::Lua55, |vm| {
            singlestep_55(vm, false).expect("a step");
        }),
    ];
    let kept: Vec<LuaVersion> = runs
        .into_iter()
        .filter(|&(version, step)| {
            let mut vm = Vm::new_minimal_with_hash_seed(version, 1);
            vm.open_all_libs().expect("the libraries open");
            let r = vm.eval(
                "collectgarbage('stop') collectgarbage()
                 x = false weak = setmetatable({}, {__mode = 'v'}) return weak",
            );
            let Value::Table(weak) = r.expect("the script runs")[0] else {
                unreachable!()
            };
            let (g, name) = (vm.globals(), Value::Str(vm.intern("x")));
            // `x`, then an object no one holds, the newest at the safe point
            let x = Value::Table(vm.heap.new_table());
            let _dead = vm.heap.new_table();
            vm.heap.safe_point();
            vm.raw_set(g, name, x).expect("stored");
            vm.raw_set(weak, Value::Int(1), x).expect("stored");
            while !sweeping(vm.heap.puc.pace.state()) {
                step(&mut vm);
            }
            vm.raw_set(g, name, Value::Nil).expect("stored");
            assert!(vm.emergency_collect());
            !weak.get_int(1).is_nil()
        })
        .map(|(version, _)| version)
        .collect();
    assert!(kept.is_empty(), "kept in {kept:?}");
}

#[test]
fn a_5_2_emergency_leaves_the_estimate_its_cycle_traversed() {
    // `luaC_fullgc` runs the cycle as steps, an emergency's as well: its
    // atomic step leaves `GCestimate`, as the program's does
    let after = |emergency: bool| {
        let mut vm = Vm::new_minimal_with_hash_seed(LuaVersion::Lua52, 1);
        vm.open_all_libs().expect("the libraries open");
        vm.eval(
            "keep = {} for i = 1, 300 do keep[i] = {i} end
             for i = 1, 300 do local _ = {i, i} end",
        )
        .expect("the script runs");
        // an emergency with no young object, which the program's
        // collection would not keep
        full_collect(&mut vm, emergency.then_some((Some(0), true)));
        vm.heap.puc.pace.estimate
    };
    assert_eq!(after(true), after(false));
}

#[test]
fn the_cycle_after_the_last_number_is_not_a_new_thread_s_mark() {
    let mut vm = Vm::new_minimal_with_hash_seed(LuaVersion::Lua53, 1);
    vm.open_all_libs().expect("the libraries open");
    vm.eval("collectgarbage('stop') collectgarbage()")
        .expect("the script runs");
    let co = match vm
        .eval("return coroutine.create(function() end)")
        .expect("made")[0]
    {
        crate::runtime::Value::Coro(c) => c,
        other => panic!("a thread: {other:?}"),
    };
    vm.heap.puc.cycle = std::num::NonZeroU32::MAX;
    restart(&mut vm);
    assert_eq!(vm.heap.puc.cycle.get(), 1);
    // PUC's traversal shrinks the new thread's stack in this cycle
    assert!(vm.heap.puc_shrinks_now(co));
    assert!(!vm.heap.puc_shrinks_now(co));
}
