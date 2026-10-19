//! The two trace tiers against the interpreter: the baseline code
//! generator alone, Cranelift alone, and the baseline tier moving traces to
//! Cranelift (at once, or partway through a run). The tier is a per-Vm
//! setting, so the settings run side by side in one process.

use luna_jit::jit::trace::TraceTier;
use luna_jit::jit_backend::trace::{baseline_codegen_count, baseline_fallback};
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use super::quarantine::with_quarantine;
use super::trace_corpus::*;

#[test]
fn the_baseline_tier_runs_the_corpus_like_the_interpreter() {
    let before = baseline_codegen_count();
    let fallback = baseline_fallback().0;
    let seen = corpus(TraceTier::Baseline, 0);
    assert!(
        baseline_codegen_count() > before,
        "no baseline code was made"
    );
    assert_eq!(
        baseline_fallback().0,
        fallback,
        "a trace fell back to Cranelift: {}",
        baseline_fallback().1
    );
    assert_eq!(seen.tiered_up, 0);
}

#[test]
fn the_optimizing_tier_runs_the_corpus_like_the_interpreter() {
    let before = baseline_codegen_count();
    corpus(TraceTier::Optimizing, 0);
    assert_eq!(
        baseline_codegen_count(),
        before,
        "Optimizing made baseline code"
    );
}

#[test]
fn moving_every_trace_to_cranelift_at_once_keeps_the_results() {
    let seen = corpus(TraceTier::Auto, 1);
    assert!(seen.tiered_up > 0, "no trace moved to Cranelift");
}

#[test]
fn moving_traces_to_cranelift_partway_keeps_the_results() {
    let seen = corpus(TraceTier::Auto, 37);
    assert!(seen.tiered_up > 0, "no trace moved to Cranelift");
}

/// The corpus again with other hash seeds, one for each dialect: where
/// collections fall moves with the seed, and what a trace holds across
/// one shows up only at some of them.
#[test]
fn moving_traces_partway_keeps_the_results_under_other_seeds() {
    let seen = corpus_seeded(TraceTier::Auto, 37, |i| 2 + i as u32);
    assert!(seen.tiered_up > 0, "no trace moved to Cranelift");
}

/// A loop recorded through `loadstring`, whose load collects (5.1's
/// chunk-name fixture): the recording held the prototype the iteration
/// before had loaded, which the collection freed, and a trace later
/// reached it through the freed memory. Over many Vms in one process, one
/// after another as the corpus runs them, the freed prototypes' memory
/// comes back to new ones and the run crashed.
#[test]
fn a_recording_through_a_load_that_collects_holds_nothing_freed() {
    Vm::check_recordings_after_collections(true);
    let f = fixtures("5.1")
        .into_iter()
        .find(|f| f.ends_with("815_debug_chunkid.lua"))
        .expect("the fixture");
    let source = std::fs::read(&f).expect("read fixture");
    for seed in 1..=64 {
        let want = run(&mut interp_seeded(LuaVersion::Lua51, seed), &source);
        for hot in [1, 3] {
            // freed blocks zeroed and kept: a recording kept through a
            // collection that freed what it holds is caught right there
            let got = with_quarantine(LuaVersion::Lua51, seed, |vm| {
                opened(vm);
                set_tiers(vm, hot, TraceTier::Auto, 37);
                run(vm, &source)
            });
            assert_eq!(got, want, "seed {seed}, hot {hot}");
        }
    }
}

/// A loop recorded through a function it loads, drops and then collects
/// in full: the recording holds the dead prototype, and the full
/// collection, which sweeps right after it marks, is where it is found
/// dead (before the sweep, not after it, when its header is freed).
#[test]
fn a_full_collection_during_a_recording_reads_nothing_freed() {
    Vm::check_recordings_after_collections(true);
    let source = b"
        local n = 0
        for i = 1, 200 do
          local f = load('return ' .. i)
          n = n + f()
          f = nil
          collectgarbage()
        end
        print(n)
    ";
    for seed in 1..=16 {
        let want = run(&mut interp_seeded(LuaVersion::Lua54, seed), source);
        for hot in [1, 3] {
            // freed blocks zeroed and kept: a recording kept through a
            // collection that freed what it holds is caught right there
            let got = with_quarantine(LuaVersion::Lua54, seed, |vm| {
                opened(vm);
                set_tiers(vm, hot, TraceTier::Auto, 37);
                run(vm, source)
            });
            assert_eq!(got, want, "seed {seed}, hot {hot}");
        }
    }
}

/// `src` returns a function; run it under the interpreter and under `vm`
/// `calls` times, comparing every result.
fn compare_calls(vm: &mut Vm, src: &str, calls: usize) {
    let mut it = interp(LuaVersion::Lua54);
    let get = |vm: &mut Vm| {
        let main = vm.load(src.as_bytes(), b"=t").expect("load");
        match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
            Value::Closure(f) => f,
            ref v => panic!("chunk returned {v:?}"),
        }
    };
    let (f, g) = (get(vm), get(&mut it));
    for k in 0..calls {
        let a = vm.call_value(Value::Closure(f), &[]).expect("trace run");
        let b = it
            .call_value(Value::Closure(g), &[])
            .expect("interpreter run");
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "call {k}");
    }
}

const COUNTING_LOOP: &str = r#"
return function()
    local t = {n = 0, s = 0.5}
    for i = 1, 20000 do
        t.n = t.n + i % 7
        if i % 3 == 0 then t.s = t.s + 1.25 end
    end
    return t.n, t.s
end
"#;

#[test]
fn a_loop_moves_to_cranelift_partway_through_one_call() {
    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, 1000);
    compare_calls(&mut vm, COUNTING_LOOP, 3);
    assert!(vm.trace_dispatched_count() > 0);
    assert!(
        vm.trace_tiered_up_count() > 0,
        "the loop stayed in the baseline tier"
    );
}

/// A generic-for trace over string keys that leaves at its head to move
/// to Cranelift: the interpreter must get the key back as a string, or
/// `next` rejects it.
#[test]
fn a_generic_for_left_at_its_head_hands_back_the_key() {
    let src = r#"
return function()
    local t = {}
    for i = 1, 200 do t["k" .. i] = i end
    local n, s = 0, 0
    for k, v in pairs(t) do
        n = n + 1
        s = s + v
    end
    return n, s
end
"#;
    for at in [3, 5, 9, 17] {
        let mut vm = tiered(LuaVersion::Lua54, 2, TraceTier::Auto, at);
        compare_calls(&mut vm, src, 3);
        assert!(
            vm.trace_tiered_up_count() > 0,
            "at {at}: nothing moved to Cranelift"
        );
    }
}

/// Self-recursion: the trace's hot exits get side traces, recorded,
/// compiled and linked into the parent's exits (the only shape the trace
/// JIT links side traces for).
const SIDE_EXITS: &str = r#"
local function f(n)
    if n < 2 then return n end
    return f(n - 1) + f(n - 2)
end
return function() return f(20) end
"#;

/// Tier-up counts on either side of the side exits' hot threshold, of the
/// side trace's recording and of its linking: the move to Cranelift lands
/// on a parent being linked and on a child just linked, and the results
/// stay the interpreter's.
#[test]
fn moving_to_cranelift_around_side_trace_linking_keeps_the_results() {
    let mut side = 0;
    let mut moved = 0;
    for at in (1..=60).chain([80, 120, 200, 400, 800, 3000]) {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.set_trace_tier(TraceTier::Auto);
        vm.set_trace_tier_up_at(at);
        compare_calls(&mut vm, SIDE_EXITS, 3);
        side += vm.trace_side_trace_compiled_count();
        moved += vm.trace_tiered_up_count();
    }
    assert!(side > 0, "no side trace was linked");
    assert!(moved > 0, "nothing moved to Cranelift");
}

const SUM_MOD: &str = r#"
return function(n)
    local s = 0
    for i = 1, n do s = s + i % 7 end
    return s
end
"#;

fn sum_mod(vm: &mut Vm, n: i64) -> i64 {
    let main = vm.load(SUM_MOD.as_bytes(), b"=t").expect("load");
    let f = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0];
    let call = |vm: &mut Vm| match vm.call_value(f, &[Value::Int(n)]).expect("call")[..] {
        [Value::Int(s), ..] => s,
        ref other => panic!("not an integer: {other:?}"),
    };
    let first = call(vm);
    assert_eq!(vm.trace_tiered_up_count(), 0, "moved within the first call");
    let second = call(vm);
    assert_eq!(first, second);
    second
}

/// A loop in a function that is called again moves to Cranelift after a
/// quarter of `tier_up_at`; one call running more iterations than that
/// but fewer than `tier_up_at` stays in the baseline tier.
#[test]
fn a_trace_moves_sooner_once_its_function_is_called_again() {
    let want = |n: i64| (1..=n).map(|i| i % 7).sum::<i64>();
    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, 1000);
    assert_eq!(sum_mod(&mut vm, 300), want(300));
    assert_eq!(
        vm.trace_tiered_up_count(),
        1,
        "the second call did not move the loop"
    );

    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, 1000);
    let main = vm.load(SUM_MOD.as_bytes(), b"=t").expect("load");
    let f = vm.call_value(Value::Closure(main), &[]).expect("chunk")[0];
    let r = vm.call_value(f, &[Value::Int(900)]).expect("call");
    assert!(matches!(r[0], Value::Int(s) if s == want(900)));
    assert!(vm.trace_dispatched_count() > 0);
    assert_eq!(
        vm.trace_tiered_up_count(),
        0,
        "one call below tier_up_at moved the loop"
    );
}

/// A side trace runs one pass per return to the dispatcher; counted there,
/// a hot one moves to Cranelift like its parent.
#[test]
fn a_hot_side_trace_moves_to_cranelift() {
    let mut side_moved = false;
    for at in [50, 200, 800] {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.set_trace_tier(TraceTier::Auto);
        vm.set_trace_tier_up_at(at);
        compare_calls(&mut vm, SIDE_EXITS, 5);
        let side = vm.trace_side_trace_compiled_count();
        let roots = vm.trace_compiled_count() - side;
        side_moved |= side > 0 && vm.trace_tiered_up_count() > roots;
    }
    assert!(side_moved, "no side trace moved to Cranelift");
}
