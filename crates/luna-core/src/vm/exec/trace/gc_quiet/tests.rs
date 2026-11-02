//! Hot counting pauses after a collection exit, and each of the three ways
//! out of the pause ends it.

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
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.jit.trace_enabled = true;
    vm.jit.trace_hot_threshold = u32::MAX;
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
    let mut vm = vm();
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

// the pause holds through the frame's own loop and the calls it makes, and
// nothing counts
#[test]
fn nothing_counts_after_a_collection_exit() {
    let src = format!("{G} mark(-1) g() g() {LOOP} return quiet(), g");
    let (q, counts) = run(&src, |_| -1);
    assert!(q);
    assert_eq!(counts, [0, 0, 0]);
    // the same without the exit counts all of it
    let src = format!("{G} g() g() {LOOP} return quiet(), g");
    let (q, counts) = run(&src, |_| -1);
    assert!(!q);
    assert!(counts.iter().all(|&c| c > 0), "{counts:?}");
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
    let prev = vm.trace_step_enter_at(1);
    assert!(vm.jit.step.quiet.is_none());
    vm.trace_step_leave(prev);
}
