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
/// rehash) leaves the trace for the interpreter, which runs the read, and
/// the loop goes on in the trace.
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
    let (_vm, f, entries) = run_both(src, &[], 3);
    // the outer loop and the one that reshapes `o`
    assert_eq!(dispatchable_heads(f).len(), 2);
    // each miss is one more entry; the loop still runs in the trace
    assert!(entries > 2 && entries < 200, "{entries} entries");
}
