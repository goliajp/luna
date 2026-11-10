//! Placing code whose pages cannot turn executable (`region::protect`
//! failing, which the kernel does when it runs out of memory for the
//! mapping): made to fail on the test's own thread.

use super::*;

thread_local! {
    pub(super) static PROTECT_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn failing<R>(f: impl FnOnce() -> R) -> R {
    PROTECT_FAILS.with(|c| c.set(true));
    let r = f();
    PROTECT_FAILS.with(|c| c.set(false));
    r
}

#[test]
fn a_fresh_arena_whose_placement_fails_keeps_nothing() {
    let mut a = CodeArena::default();
    assert!(failing(|| a.place(&[0xc3; 32])).is_err());
    // dropped now, the arena would leave no mapping and no list behind
    assert!(a.chunks.is_empty());
    assert_eq!(a.chunks.capacity(), 0);
    assert_eq!(a.left, 0);
    assert!(a.place(&[0xc3; 32]).is_ok());
    assert_eq!(a.chunks.len(), 1);
}

#[test]
fn a_placement_failing_in_a_new_chunk_keeps_the_chunks_with_code() {
    let mut a = CodeArena::default();
    assert!(a.place(&[0xc3; 32]).is_ok());
    let big = vec![0xc3u8; CHUNK + 1];
    assert!(failing(|| a.place(&big)).is_err());
    assert_eq!(a.chunks.len(), 1);
    assert_eq!(a.left, 0);
    assert!(a.place(&[0xc3; 32]).is_ok());
    assert_eq!(a.chunks.len(), 2);
}
