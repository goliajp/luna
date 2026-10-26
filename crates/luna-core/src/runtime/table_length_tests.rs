//! The array part's counts and the length shortcut, checked against a
//! search for a border after every change.

use super::*;

/// `len` without the `acount` / `aprefix` shortcut: the search of the
/// table's dialect, which every shortcut answer must equal. Taken after
/// `len`, so a 5.5 table's search starts from the hint `len` left, which
/// is the border itself.
fn len_by_search(t: &Table) -> i64 {
    match t.dialect() {
        Dialect::L55 => t.len_55(),
        Dialect::L54 => t.len_54(),
        d => t.len_51(d),
    }
}

fn check_counts(t: &Table) {
    let atags = t.atags();
    let count = atags.iter().filter(|&&g| g != raw::NIL).count() as u32;
    let run = atags
        .iter()
        .position(|&g| g == raw::NIL)
        .unwrap_or(atags.len()) as u32;
    assert_eq!(t.acount, count);
    assert!(t.aprefix <= run, "aprefix {} past the run {run}", t.aprefix);
    let n = t.len();
    assert_eq!(n, len_by_search(t));
}

// the prefix may lag behind the run (a refill scans only 64 slots
// ahead, a method-JIT inline store extends it by one slot); growing
// the array part brings it up to the run again
#[test]
fn growing_catches_up_a_lagging_prefix() {
    with_table(|heap, t| {
        for i in 1..=8 {
            let _ = t.set_int(heap, i, Value::Int(i));
        }
        let asize = t.asize();
        assert_eq!(t.acount as usize, asize);
        t.aprefix = 1;
        t.resize(heap, asize * 2, 0).unwrap();
        check_counts(t);
        assert_eq!(t.aprefix as usize, asize);
    });
    with_table(|heap, t| {
        for i in 1..=8 {
            let _ = t.set_int(heap, i, Value::Int(i));
        }
        let _ = t.set_int(heap, 6, Value::Nil);
        let asize = t.asize();
        t.aprefix = 1;
        t.resize(heap, asize * 2, 0).unwrap();
        check_counts(t);
        assert_eq!(t.aprefix, 5);
    });
}

#[test]
fn length_shortcut_matches_the_border_search() {
    // xorshift, so the sequence is the same on every run
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    // every change of every table is made; under Miri, where the checks
    // take most of the time, every 16th state is checked
    let every = if cfg!(miri) { 16 } else { 1 };
    let mut step = 0;
    let mut check = |t: &Table| {
        step += 1;
        if step % every == 0 {
            check_counts(t);
        }
    };
    for _ in 0..200 {
        with_table(|heap, t| {
            for _ in 0..300 {
                let k = next(80) as i64 + 1;
                let v = if next(4) == 0 {
                    Value::Nil
                } else {
                    Value::Int(k)
                };
                let _ = t.set_int(heap, k, v);
                check(t);
            }
            // appends and pops at the border, as `t[#t + 1] = v` does
            for _ in 0..100 {
                let n = t.len();
                if next(3) == 0 && n > 0 {
                    let _ = t.set_int(heap, n, Value::Nil);
                } else {
                    let _ = t.set_int(heap, n + 1, Value::Int(n));
                }
                check(t);
            }
        });
    }
}
