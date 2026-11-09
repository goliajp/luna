//! The baseline tier's call stub leaving a link to the Vm before it takes
//! anything, for each reason its run words give, noted where
//! `JitCounters::links_left_by_stub` counts it from.

use super::run;
use crate::jit_backend::trace::lir::CodeArena;
use crate::jit_backend::trace::lir::stub::alone::{LEFT, link_alone};
use luna_core::__private::{LINK_DEPTH, RunWords, SideCell};

/// Writes the 32 bits at byte `at` of `words`.
fn set32(words: &mut [u64], at: i32, v: u32) {
    let (w, shift) = (at as usize / 8, at as usize % 8 * 8);
    words[w] = words[w] & !(0xffff_ffff << shift) | u64::from(v) << shift;
}

/// The 32 bits at byte `at` of `words`.
fn get32(words: &[u64], at: i32) -> u32 {
    (words[at as usize / 8] >> (at as usize % 8 * 8)) as u32
}

/// The stub run on a cell with words for the link (`fast`) or none, and run
/// words with `set` written: what it returns, and the reason it noted.
fn taken(fast: bool, set: &[(i32, u32)]) -> (i64, u32) {
    let (rw, at) = (RunWords::offsets(), SideCell::offsets());
    let mut words = vec![0u64; size_of::<RunWords>().div_ceil(8)].into_boxed_slice();
    for &(off, v) in set {
        set32(&mut words, off, v);
    }
    let link_words = [0u64; 32];
    let mut cell = vec![0u64; size_of::<SideCell>().div_ceil(8)].into_boxed_slice();
    cell[at.words as usize / 8] = words.as_ptr() as u64;
    if fast {
        cell[at.link_fast as usize / 8] = link_words.as_ptr() as u64;
    }
    // an arena dropped leaves its code mapped, as the function needs
    let code = (CodeArena::default())
        .place(&link_alone(cell.as_ptr() as usize))
        .expect("placed");
    let (_, r) = run(code, [0; 4]);
    (r, get32(&words, rw.stub_left))
}

#[test]
fn the_stub_leaves_links_for_what_its_run_words_say() {
    let rw = RunWords::offsets();
    let depth = LINK_DEPTH as u32;
    let room = (rw.trail_cap, 4 * depth);
    assert_eq!(taken(false, &[room]), (LEFT, 0), "no words");
    let cases: [(&[(i32, u32)], u32); 6] = [
        (&[room, (rw.blocked, 1)], 1),
        (&[room, (rw.blocked, 1 << 24)], 1),
        (&[room, (rw.quiet, 1)], 2),
        (&[room, (rw.trail_len, depth + 1), (rw.link_base, 1)], 3),
        (&[(rw.trail_len, 2), (rw.trail_cap, 2)], 4),
        (
            &[
                (rw.trail_len, depth),
                (rw.link_base, 1),
                (rw.trail_cap, depth),
            ],
            4,
        ),
    ];
    for (set, why) in cases {
        assert_eq!(taken(true, set), (LEFT, why), "{set:?}");
    }
}
