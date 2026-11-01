//! Stores that land on a jump target: after an `if ... end` and at a loop's
//! exit.

use super::*;

/// The statement after an `if ... end` starts at the `if`'s skip target;
/// PUC still writes its result straight into the local (`discharge2reg`
/// does not look at `fs->lasttarget`).
#[test]
fn retarget_at_a_jump_target_after_if() {
    let src = "local n, c = 0, ... if c then n = n + 5 end n = n + 1 return n";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 0, "{code:?}");
    assert!(
        code.iter()
            .filter(|i| i.op() == Op::AddI)
            .all(|i| i.a() == 0)
    );
    for (c, want) in [("true", 6), ("false", 1)] {
        let src = format!("local n, c = 0, {c} if c then n = n + 5 end n = n + 1 return n");
        assert_eq!(eval_int_all(&src), [want; 5], "{src}");
    }
}

/// `k = #t` right after an `if ... end`: the `Len` lands in `k`.
#[test]
fn retarget_len_at_a_jump_target() {
    let src = "local t, k = {1, 2, 3}, 0 if k == 0 then k = k + 1 end k = #t return k";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 0, "{code:?}");
    assert_eq!(eval_int_all(src), [3; 5]);
}

/// `w = v` right after a numeric `for`: the loop's exit lands on the
/// temporary Move, which is dropped; the store reads `v` directly.
#[test]
fn rhs_elision_at_a_loop_exit() {
    let src = "local w, v = 0, 7 for i = 1, 3 do v = v + i end w = v return w * 100 + v";
    let code = compile_main(src);
    assert_eq!(count_moves(&code), 1, "{code:?}");
    assert_eq!(eval_int_all(src), [1313; 5]);
}
