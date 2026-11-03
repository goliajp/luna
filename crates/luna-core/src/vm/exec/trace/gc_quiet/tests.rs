//! Hot counting pauses after a collection exit, and each of the ways out
//! of the pause ends it.

use crate::runtime::{LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::Args;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;

/// `mark(head)`: the calling frame's trace left for a collection, its head
/// at pc `head` (-1: none the frame reaches).
fn mark(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let head = match Args::new(fs, nargs).get(vm, 0) {
        Value::Int(h) if h >= 0 => h as u32,
        _ => u32::MAX,
    };
    let depth = vm.frames.len();
    vm.gc_quiet_from(depth, head);
    Ok(0)
}

/// `quiet()`: whether hot counting still pauses.
fn quiet(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let q = vm.jit.step.quiet.is_some();
    vm.nat_ret1(fs, Value::Bool(q))
}

/// A 5.4 Vm counting hot loops and calls, with `mark` and `quiet`, and
/// thresholds no run here reaches: nothing records.
fn vm() -> Vm {
    vm_at(u32::MAX)
}

/// [`vm`] with loops hot at `threshold`.
fn vm_at(threshold: u32) -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.jit.trace_enabled = true;
    vm.jit.trace_hot_threshold = threshold;
    vm.jit.call_hot_threshold = u32::MAX;
    let (m, q) = (vm.native(mark), vm.native(quiet));
    vm.set_global("mark", m).unwrap();
    vm.set_global("quiet", q).unwrap();
    vm
}

/// The head of the first numeric `for` loop of `cl`'s own code.
fn loop_head(cl: crate::runtime::Gc<LuaClosure>) -> i64 {
    let code = &cl.proto.code;
    let pc = code
        .iter()
        .position(|i| i.op() == Op::ForLoop)
        .expect("a numeric for");
    pc as i64 + 1 - code[pc].bx() as i64
}

/// Runs `src` with global `HEAD` set to `head`: what it returns, and the
/// back-edges and calls counted on the chunk and on the function `g` it
/// returns second.
fn run(src: &str, head: impl Fn(crate::runtime::Gc<LuaClosure>) -> i64) -> (bool, [u32; 3]) {
    run_in(&mut vm(), src, head)
}

/// [`run`] on `vm`.
fn run_in(
    vm: &mut Vm,
    src: &str,
    head: impl Fn(crate::runtime::Gc<LuaClosure>) -> i64,
) -> (bool, [u32; 3]) {
    let f = vm.load(src.as_bytes(), b"=t").unwrap();
    vm.set_global("HEAD", head(f)).unwrap();
    let r = vm.call_value(Value::Closure(f), &[]).unwrap();
    let Value::Closure(g) = r[1] else {
        panic!("no function returned")
    };
    let counts = [
        f.proto.trace_hot_count(),
        g.proto.trace_hot_count(),
        g.proto.call_hot_count(),
    ];
    (matches!(r[0], Value::Bool(true)), counts)
}

const LOOP: &str = "local s = 0 for i = 1, 100 do s = s + i end";
const G: &str = "local function g() local s = 0 for i = 1, 100 do s = s + i end return s end";

// inside the trace's loop the pause holds through the calls the frame
// makes and the loops they run, and nothing counts
#[test]
fn nothing_counts_in_the_loop_after_a_collection_exit() {
    let body = "g() g() P = quiet()";
    let src = format!("{G} for i = 1, 1 do mark(HEAD) {body} end return P, g");
    let (q, counts) = run(&src, loop_head);
    assert!(q);
    assert_eq!(counts, [0, 0, 0]);
    // the same without the exit counts all of it
    let src = format!("{G} for i = 1, 1 do {body} end return P, g");
    let (q, counts) = run(&src, loop_head);
    assert!(!q);
    assert!(counts[1..].iter().all(|&c| c > 0), "{counts:?}");
}

// a call the frame makes once the trace's loop is over (or with no loop of
// the frame's to go back to) ends the pause, and the callee counts as it
// would have without the exit
#[test]
fn a_call_after_the_loop_ends_the_pause() {
    let src = format!("{G} for i = 1, 1 do mark(HEAD) end g() return quiet(), g");
    let (q, counts) = run(&src, loop_head);
    assert!(!q);
    let (_, unpaused) = run(&format!("{G} g() return quiet(), g"), |_| -1);
    assert_eq!(counts[1..], unpaused[1..]);
    let (q, counts) = run(&format!("{G} mark(-1) g() return quiet(), g"), |_| -1);
    assert!(!q);
    assert_eq!(counts[1..], unpaused[1..]);
}

// a long call inside the loop: once it has held back as many back-edges as
// make a loop hot, the pause ends, and the callee goes on to record
#[test]
fn a_long_call_in_the_loop_ends_the_pause_and_records() {
    let src = format!("{G} for i = 1, 1 do mark(HEAD) g() P = quiet() end return P, g");
    let mut vm = vm_at(30);
    let (q, [_, inner, _]) = run_in(&mut vm, &src, loop_head);
    assert!(!q);
    assert!(inner > 0, "{inner}");
    let c = &vm.jit.counters;
    assert!(c.closed + c.aborted > 0, "nothing recorded");
}

// a back-edge of the frame the exit left that goes elsewhere than the
// trace's head: the trace's loop is over, and that back-edge on counts as
// it would have without the exit
#[test]
fn the_pause_ends_at_another_back_edge_of_the_frame() {
    let (q, [main, _, _]) = run(&format!("{G} mark(-1) {LOOP} return quiet(), g"), |_| -1);
    assert!(!q);
    let (_, [unpaused, _, _]) = run(&format!("{G} {LOOP} return quiet(), g"), |_| -1);
    assert_eq!(main, unpaused);
    // nested in the frame's loop, the call's own back-edges leave it paused
    let src = format!("{G} for i = 1, 1 do mark(HEAD) g() P = quiet() end {LOOP} return P, g");
    let (q, [main, inner, _]) = run(&src, loop_head);
    assert!(q);
    assert_eq!((main, inner), (unpaused, 0));
}

// the frame the exit left returns: the frame below counts again
#[test]
fn the_pause_ends_when_the_frame_returns() {
    let src = format!("{G} local function h() mark(-1) end h() {LOOP} return quiet(), g");
    let (q, [main, _, _]) = run(&src, |_| -1);
    assert!(!q);
    assert!(main > 0);
}

// the loop reaches the head of the trace that left: from that back-edge on
// it counts as it would have without the exit
#[test]
fn the_pause_ends_at_the_head_of_the_trace() {
    let src = format!("{G} mark(HEAD) {LOOP} return quiet(), g");
    let (q, [main, _, _]) = run(&src, loop_head);
    assert!(!q);
    let (_, [unpaused, _, _]) = run(&format!("{G} {LOOP} return quiet(), g"), |_| -1);
    assert_eq!(main, unpaused);
}

// entering any trace ends it
#[test]
fn the_pause_ends_when_a_trace_is_entered() {
    let mut vm = vm();
    let f = vm.load(b"mark(-1) return quiet()", b"=t").unwrap();
    let r = vm.call_value(Value::Closure(f), &[]).unwrap();
    assert!(matches!(r[0], Value::Bool(true)));
    let prev = vm.trace_step_enter(std::ptr::NonNull::dangling().as_ptr());
    assert!(vm.jit.step.quiet.is_none());
    vm.trace_step_leave(prev);
}

// during the pause the loop counts are parked untouched and the stand-in
// table stays at the threshold in every slot; the end of the pause puts
// them back
#[test]
fn the_loop_counts_are_parked_during_a_pause() {
    let mut vm = vm();
    let src = format!("{G} for i = 1, 1 do mark(HEAD) g() P = quiet() end return P");
    let f = vm.load(src.as_bytes(), b"=t").unwrap();
    vm.set_global("HEAD", loop_head(f)).unwrap();
    let r = vm.call_value(Value::Closure(f), &[]).unwrap();
    assert!(matches!(r[0], Value::Bool(true)));
    assert!(vm.jit.loop_hot.iter().all(|&c| c == u32::MAX));
    let parked = vm.jit.step.parked_hot.as_ref().expect("parked");
    assert!(parked.iter().all(|&c| c == 0));
    let prev = vm.trace_step_enter(std::ptr::NonNull::dangling().as_ptr());
    vm.trace_step_leave(prev);
    assert!(vm.jit.loop_hot.iter().all(|&c| c == 0));
}
