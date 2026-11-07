//! Allocations that fail in a trace whose loop writes what running a pass
//! again would show: the trace leaves at the op that could not allocate,
//! or the dispatcher makes what an exit could not, and nothing the trace
//! did before is done again.

use super::*;
use luna_jit::jit::trace::TraceTier;

/// A loop that appends: running any of its passes again would show.
const APPEND: &str = "local t = {} for i = 1, 3000 do t[#t + 1] = {i} end \
     local c = 0 for j = 1, #t do c = c + t[j][1] end return #t * 100000 + c";

/// A table the trace sinks, made at the exit that takes the branch, after
/// a count kept in a table.
const SUNK: &str = "local cnt = {0} local s = 0 \
     for i = 1, 3000 do local p = {i, i * 2} cnt[1] = cnt[1] + 1 \
       if i % 5 == 0 then s = s + p[2] end end \
     return cnt[1] * 10000000 + s";

/// Tables held in tables: one in another, and two in each other. A sunk
/// table stored in another escapes, so neither is a sunk table's value.
const NESTED: [&str; 2] = [
    "local cnt = {0} local s = 0 \
     for i = 1, 3000 do local p = {i} local q = {p, i} cnt[1] = cnt[1] + 1 \
       if i % 5 == 0 then s = s + q[1][1] + q[2] end end \
     return cnt[1] * 10000000 + s",
    "local cnt = {0} local s = 0 \
     for i = 1, 3000 do local p = {i} local q = {p, i} p[2] = q cnt[1] = cnt[1] + 1 \
       if i % 5 == 0 then s = s + p[2][2] + q[1][1] end end \
     return cnt[1] * 10000000 + s",
];

/// A table made inside a function the trace inlines: an exit there
/// rebuilds the function's frame.
const INLINED_ALLOC: &str = "local cnt = {0} local t = {} for i = 1, 64 do t[i] = false end \
     local function g(i) return {i} end \
     for i = 1, 3000 do cnt[1] = cnt[1] + 1 t[i % 64 + 1] = g(i) end \
     local s = 0 for i = 1, 64 do s = s + t[i][1] end return cnt[1] * 10000000 + s";

/// A string the trace accumulates, past what compiled code makes (256KB),
/// after a count kept in a table.
const ACCUM_LONG: &str = "local cnt = {0} local piece = string.rep('x', 1024) local s = '' \
     for i = 1, 400 do cnt[1] = cnt[1] + 1 s = s .. piece end \
     return cnt[1] * 1000000 + #s";

/// A string a trace accumulates, past `tier_up_at` passes: each entry
/// after the first moves on to the optimizing tier's loop at its first
/// back edge, making the string before it does (the pieces are long, so
/// the string is new, not one a concatenation made already).
const ACCUM_TIERED: &str = "local cnt = {0} local piece = string.rep('x', 64) local s = '' \
     for i = 1, 400 do cnt[1] = cnt[1] + 1 s = s .. piece end \
     return cnt[1] * 1000000 + #s";

/// A loop calling a function whose frame the trace rebuilds at an exit
/// inside it, run in fresh coroutines: the trace compiled in the first
/// runs in the next from their start, on a stack that has not yet held the
/// function's registers (in 5.1 it has to grow for them at the exit).
/// `tail` ends the chunk, with `all` running the loops.
pub(super) fn frames_prog(tail: &str) -> String {
    let decl = |p: &str, n: usize, v: &str| {
        let names: Vec<String> = (1..=n).map(|i| format!("{p}{i}")).collect();
        format!("local {} = {}", names.join(", "), vec![v; n].join(", "))
    };
    format!(
        "local cnt = {{0, 0}} \
         local function f(x) {} if x % 3 == 0 then return x + 1 end return x end \
         local function run() {} local s = 0 \
           for i = 1, 300 do cnt[1] = cnt[1] + 1 cnt[2] = i s = s + f(i) end return s end \
         local function all() local s = 0 for k = 1, 10 do s = s + coroutine.wrap(run)() end \
           return s end {tail}",
        decl("a", 20, "x"),
        decl("b", 5, "0")
    )
}

/// The result, the side trace runs, whether no deopt is left parked, and
/// the frames, sunk tables and strings the dispatcher made for exits.
fn traced(v: LuaVersion, p: Option<Refuse>, src: &str) -> (String, u64, bool, [u64; 3]) {
    let mut vm = vm_with(v, p, true);
    vm.set_jit_enabled(false);
    // the first coroutine, not the first pass, compiles the frames' trace
    if src.contains("coroutine.wrap(run)") {
        vm.set_trace_hot_threshold(64);
        vm.set_call_hot_threshold(64);
    }
    let r = result(&mut vm, src);
    let c = vm.jit_unmade_counts();
    let made = [c.frames, c.sunk, c.accum];
    (
        r,
        vm.trace_side_trace_run_count(),
        vm.jit().pending_err.is_none(),
        made,
    )
}

/// Refuses each of up to `max` of the growths of `kind` the program's
/// traces make, one per run, and checks the result and that no deopt is
/// left parked; `side` asks for the growths to be made in side traces.
/// What the dispatcher made, summed over the runs.
fn sweep(v: LuaVersion, src: &str, kind: Option<BlockKind>, max: usize, side: bool) -> [u64; 3] {
    let want = interpreted(v, src);
    let all = Refuse::new(kind, usize::MAX, true);
    let seen = all.seen.clone();
    let (r, runs, clear, _) = traced(v, Some(all), src);
    assert_eq!(r, want, "{v:?}: {src}");
    assert!(clear, "{v:?}: a deopt left parked: {src}");
    if side {
        assert!(runs > 0, "{v:?}: no side trace ran: {src}");
    }
    let n = seen.load(Relaxed);
    let mut made = [0; 3];
    for k in spread(n, max) {
        let p = Refuse::new(kind, k, true);
        let refused = p.refused.clone();
        let (r, _, clear, m) = traced(v, Some(p), src);
        assert_eq!(r, want, "{v:?} #{k}: {src}");
        assert_eq!(refused.load(Relaxed), 1, "{v:?} #{k}: {src}");
        assert!(clear, "{v:?} #{k}: a deopt left parked: {src}");
        for (a, b) in made.iter_mut().zip(m) {
            *a += b;
        }
    }
    made
}

#[test]
fn a_trace_that_cannot_allocate_runs_no_pass_again() {
    for v in VERSIONS {
        let all = Refuse::new(Some(BlockKind::Table), usize::MAX, true);
        let seen = all.seen.clone();
        traced(v, Some(all), APPEND);
        assert!(seen.load(Relaxed) > 0, "{v:?}: no trace allocated");
        sweep(v, APPEND, Some(BlockKind::Table), 12, false);
    }
}

#[test]
fn a_sunk_table_an_exit_cannot_make_is_made_by_the_dispatcher() {
    // 5.5's constructor is not sunk
    let made: u64 = VERSIONS
        .into_iter()
        .map(|v| sweep(v, SUNK, Some(BlockKind::Table), 12, false)[1])
        .sum();
    assert!(made > 0, "the dispatcher made no sunk table");
}

#[test]
fn tables_held_in_tables_agree() {
    for v in VERSIONS {
        for src in NESTED {
            sweep(v, src, Some(BlockKind::Table), 12, false);
        }
    }
}

#[test]
fn a_trace_leaving_inside_an_inlined_function_rebuilds_its_frame() {
    for v in VERSIONS {
        sweep(v, INLINED_ALLOC, Some(BlockKind::Table), 12, false);
    }
}

#[test]
fn inlined_frames_an_exit_cannot_push_are_pushed_by_the_dispatcher() {
    let src = frames_prog("local s = all() return cnt[1] * 10000000 + s");
    // only 5.1's coroutines start on a stack too short for the frames
    let made: u64 = VERSIONS
        .into_iter()
        .map(|v| sweep(v, &src, Some(BlockKind::Other), 12, false)[0])
        .sum();
    assert!(made > 0, "the dispatcher pushed no frame");
}

#[test]
fn a_string_an_exit_cannot_make_is_made_by_the_dispatcher() {
    for v in VERSIONS {
        let want = interpreted(v, ACCUM_LONG);
        let (r, _, clear, made) = traced(v, None, ACCUM_LONG);
        assert_eq!(r, want, "{v:?}");
        assert!(clear, "{v:?}: a deopt left parked");
        assert!(made[2] > 0, "{v:?}: the dispatcher made no string");
        // the string made, the strings counted as PUC makes them, the
        // buffer grown (for the string the trace enters with, too)
        for kind in [BlockKind::Str, BlockKind::Other] {
            let made = sweep(v, ACCUM_LONG, Some(kind), 12, false);
            assert!(made[2] > 0, "{v:?} {kind:?}: the dispatcher made no string");
        }
    }
}

/// The result and the traces moved to the optimizing tier, refusing the
/// `k`th string compiled code makes (none for `None`).
fn tiered_run(v: LuaVersion, k: Option<usize>) -> (String, u64, usize) {
    let p = Refuse::new(Some(BlockKind::Str), k.unwrap_or(usize::MAX), true);
    let seen = p.seen.clone();
    let mut vm = vm_with(v, Some(p), true);
    vm.set_jit_enabled(false);
    vm.set_trace_tier(TraceTier::Auto);
    vm.set_trace_tier_up_at(16);
    let r = result(&mut vm, ACCUM_TIERED);
    assert!(
        vm.jit().pending_err.is_none(),
        "{v:?} {k:?}: a deopt left parked"
    );
    (r, vm.trace_tiered_up_count(), seen.load(Relaxed))
}

#[test]
fn a_string_not_made_at_the_move_to_the_optimizing_tier_leaves_at_the_loop_head() {
    for v in VERSIONS {
        let want = interpreted(v, ACCUM_TIERED);
        let (r, tiered, n) = tiered_run(v, None);
        assert_eq!(r, want, "{v:?}");
        assert!(tiered > 0, "{v:?}: no trace moved on");
        // every string compiled code makes, the ones at each move among them
        for k in 1..=n {
            assert_eq!(tiered_run(v, Some(k)).0, want, "{v:?} #{k}");
        }
    }
}
