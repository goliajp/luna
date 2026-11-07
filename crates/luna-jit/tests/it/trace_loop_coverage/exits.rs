//! Where a loop leaves its trace: the other way of a comparison stays in
//! it, a read that misses leaves it and the loop comes back.

use super::*;

#[test]
fn a_comparison_s_other_branch_runs_inside_the_trace() {
    // whichever way the recorded pass went, the other way rejoins in the
    // trace instead of leaving it
    let src = "
        return function(n)
            local last3, last5, big = 0, 0, 0
            for i = 1, n do
                if i % 3 == 0 then last3 = i end
                if i % 5 ~= 0 then last5 = i end
                if i < 7 then big = i end
            end
            return last3, last5, big
        end";
    let (_vm, f, entries) = run_both(src, &[Value::Int(1000)], 3);
    assert_eq!(dispatchable_heads(f).len(), 1);
    assert_eq!(entries, 1);
}

/// An inline array or field read whose slot no longer holds what the
/// trace was recorded with (a key past the array part, a field moved by a
/// rehash) leaves the trace, and the loop goes on in the trace.
#[test]
fn an_inline_read_that_misses_leaves_the_trace_and_comes_back() {
    let src = "
        return function()
            local t = {1, 2, 3, 4, 5, 6, 7, 8}
            local o = {a = 1}
            local s = 0
            for i = 1, 400 do
                local v = t[i % 10 + 1]
                if v then s = s + v end
                s = s + o.a
                if i == 200 then
                    for k = 1, 20 do o['k' .. k] = k end
                end
            end
            return s
        end";
    let (mut vm, f, _) = run_both(src, &[], 3);
    let relinks =
        |f: Gc<LuaClosure>| -> u64 { f.proto.traces().iter().map(|t| t.side_relinks()).sum() };
    let (entries0, relinks0) = (vm.trace_dispatched_count(), relinks(f));
    call(&mut vm, f, &[]);
    let entries = vm.trace_dispatched_count() - entries0;
    let relinked = relinks(f) - relinks0;
    // the outer loop only: the side trace from the exit at `i == 200`
    // runs the loop that reshapes `o` and goes on in the outer loop's trace
    assert_eq!(dispatchable_heads(f).len(), 1);
    // each array read that misses (`t[9]`, `t[10]`) leaves for the
    // interpreter (the side trace recorded there cannot type the nil the
    // read gives, so it never runs), and the trace is entered again
    let misses = 400 / 10 * 2;
    assert_eq!(entries, misses + 1);
    // after `i == 200` the read of the reshaped `o.a` misses at every pass,
    // and on each pass whose array read hits the side trace recorded at it
    // goes on in the loop, but at the last, where the loop ends; and the
    // side trace from `i == 200` once
    assert_eq!(relinked, (200 - 200 / 10 * 2 - 1) + 1);
}
