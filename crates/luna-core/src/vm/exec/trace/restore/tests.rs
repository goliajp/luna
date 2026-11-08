//! A register given a kind past the first back edge is restored only for a
//! call that counted its passes in its own words: the words past the window
//! keep whatever an earlier dispatch left there.

use crate::__private::{CompiledTrace, CompiledTraceParts, PassCount, TracePlans};
use crate::jit::send_compat::{TArc, TCellU32};
use crate::jit::trace::{ENTRY_TAG_ANY, ExitTag, ExitTagTable};
use crate::jit::trace_types::tier_up::TierUp;
use crate::runtime::{LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;

/// `f(x)`: the traces start at the call (`y = 1`) and leave at the return,
/// so `x` is register 0; called with no argument it holds nil.
const F: &str = "local function f(x) local y = 1; return x end return f";
const WINDOW: usize = 2;
const HEAD: u32 = 0;
const RESUME: i64 = 1;

/// Both traces' code. Called with `x` the integer 1 (the first trace), it
/// leaves this call's mark of a pass into the loop set, as a trace that
/// counts in its words and went round its back edge does; called with `x`
/// nil (the second), it writes register 0 and leaves before the write
/// counts: the exit's tag for it is `Untouched`.
unsafe extern "C" fn trace_code(reg_state: *mut i64) -> i64 {
    // SAFETY: the dispatcher hands a buffer of the window and the three
    // words past it
    unsafe {
        if *reg_state == 1 {
            *reg_state.add(crate::__private::trace_passed_slot(WINDOW)) = 1;
        } else {
            *reg_state = 777;
        }
    }
    RESUME
}

fn parts(entry: crate::jit::trace::TraceFn, pass: PassCount, tiered: bool) -> CompiledTraceParts {
    let any = [ENTRY_TAG_ANY; WINDOW];
    let none = ExitTagTable::default;
    let tags = [ExitTag::Untouched; WINDOW];
    CompiledTraceParts {
        head_pc: HEAD,
        entry,
        n_ops: 1,
        dispatchable: true,
        window_size: WINDOW as u32,
        plans: TracePlans::new(&any, &tags, none(), none(), pass),
        per_exit_inline: TArc::from(Vec::new()),
        exit_hit_counts: TArc::from(vec![TCellU32::new(0)]),
        is_inline_abort_close: false,
        dispatch_off_reason: None,
        sinkable_sites_seen: 0,
        accum_bufferable_seen: 0,
        sunk_alloc_seen: 0,
        materialize_emit_count: 0,
        closure_seen: 0,
        inline_kinds: 0,
        inline_depth: 0,
        body_writes: Box::new([]),
        downrec_link: None,
        downrec_multi_way_count: 0,
        // a count no run here reaches
        tier_up: tiered.then(|| Box::new(TierUp::new(Box::new(TCellU32::new(0)), u32::MAX, 0))),
        side_cell_exits: std::sync::Arc::new([]),
        side_cells: None,
        calls_source: None,
        link_pc: None,
        loop_tags: None,
    }
}

fn function(vm: &mut Vm) -> crate::runtime::Gc<LuaClosure> {
    let main = vm.load(F.as_bytes(), b"=t").unwrap();
    let r = vm.call_value(Value::Closure(main), &[]).unwrap();
    let Value::Closure(f) = r[0] else {
        panic!("no function returned")
    };
    let code = &f.proto.code;
    assert_eq!(f.proto.max_stack as usize, WINDOW);
    assert_eq!(code[HEAD as usize].op(), Op::LoadI);
    assert!(matches!(
        code[RESUME as usize].op(),
        Op::Return1 | Op::Return
    ));
    f
}

/// Runs a trace that leaves the mark of a pass set, then one whose plans
/// give register 0 a kind past the first back edge (`tiered`: with a
/// tier-up count), which writes a payload there and leaves before the
/// write counts: what the second function returns.
fn after_a_mark(tiered: bool) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.jit.trace_enabled = true;
    vm.jit.trace_hot_threshold = u32::MAX;
    vm.jit.call_hot_threshold = u32::MAX;
    let (marker, f) = (function(&mut vm), function(&mut vm));
    let kinds = PassCount::PerCall(Box::new([(0, ExitTag::Int)]));
    // SAFETY: both entries keep to the window and the words past it, and
    // the plans are made from the tags they are run against
    let (a, b) = unsafe {
        (
            CompiledTrace::from_parts(parts(trace_code, PassCount::Shared, false)),
            CompiledTrace::from_parts(parts(trace_code, kinds, tiered)),
        )
    };
    super::super::cache::cache_trace(marker.proto, a);
    super::super::cache::cache_trace(f.proto, b);
    let r = vm
        .call_value(Value::Closure(marker), &[Value::Int(1)])
        .unwrap();
    assert!(matches!(r[..], [Value::Int(1)]), "{r:?}");
    let r = vm.call_value(Value::Closure(f), &[]).unwrap();
    assert_eq!(vm.jit.counters.dispatched, 2, "both traces ran");
    r
}

// a call of a trace counting in its words starts with the mark cleared
#[test]
fn a_call_counting_in_its_words_starts_unmarked() {
    let r = after_a_mark(true);
    assert!(
        matches!(r[..], [Value::Nil]),
        "register 0 is the frame's nil: {r:?}"
    );
}

// Plans counting per call on a trace with no tier-up count, against the
// contract: a call of it never sets the words, so the mark an earlier trace
// left must not make its exit write register 0 back. Debug builds refuse
// such a trace.
#[cfg(not(debug_assertions))]
#[test]
fn a_mark_left_by_another_trace_restores_nothing() {
    let r = after_a_mark(false);
    assert!(
        matches!(r[..], [Value::Nil]),
        "register 0 is the frame's nil: {r:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "code counting per call has no tier-up count")]
fn plans_counting_per_call_want_a_tier_up_count() {
    let _ = after_a_mark(false);
}
