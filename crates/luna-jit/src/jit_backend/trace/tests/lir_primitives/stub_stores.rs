//! The baseline tier's call stub taking a link stores to the stack what the
//! exit's write-back would (`Value::pack_into`), for each exit tag.

use super::{FILL, run};
use crate::jit_backend::trace::lir::{CodeArena, stub::link::stores_alone};
use luna_core::__private::{
    RunWords, STACK_PTR_OFFSET, SideCell, link_fast_offsets, slot_store_reference, store_items,
};
use luna_core::runtime::value::tag;

/// Room for every field the stub reads, at its offset.
fn words_at(len: usize, set: &[(i32, u64)]) -> Box<[u64]> {
    let mut b = vec![0u64; len].into_boxed_slice();
    for &(at, v) in set {
        b[at as usize / 8] = v;
    }
    b
}

#[test]
fn the_stub_stores_what_the_exit_write_back_writes() {
    let payloads = [
        0,
        1,
        2,
        -1,
        i64::MIN,
        0x1234_5678_9abc_def0,
        1.5f64.to_bits() as i64,
    ];
    let (lf, rw, at) = (
        link_fast_offsets(),
        RunWords::offsets(),
        SideCell::offsets(),
    );
    // an arena dropped leaves its code mapped, as the functions here need
    let mut arena = CodeArena::default();
    for d in 0..9u8 {
        let items = store_items(&[(0, d)]);
        let mut fast = vec![0u32; lf.items as usize / 4];
        fast[lf.stores as usize / 4] = (items.len() / 3) as u32;
        fast.extend(&items);
        for r in payloads {
            let mut stack = [FILL as u64; 2];
            let vec = words_at(4, &[(STACK_PTR_OFFSET, stack.as_mut_ptr() as u64)]);
            let words = words_at(32, &[(rw.stack, vec.as_ptr() as u64)]);
            let cell = words_at(
                64,
                &[
                    (at.link_fast, fast.as_ptr() as u64),
                    (at.words, words.as_ptr() as u64),
                ],
            );
            let code = arena
                .place(&stores_alone(cell.as_ptr() as usize))
                .expect("placed");
            run(code, [r, 0, 0, 0]);
            let Some((t, payload)) = slot_store_reference(d, r) else {
                assert_eq!(stack, [FILL as u64; 2], "exit tag {d} leaves the slot");
                continue;
            };
            assert_eq!(
                stack[0] as u8, t,
                "tag byte of exit tag {d}, payload {r:#x}"
            );
            match payload {
                Some(b) if t == tag::BOOL => assert_eq!(stack[1] as u8 as u64, b, "bool {r:#x}"),
                Some(w) => assert_eq!(stack[1], w, "payload of exit tag {d}"),
                None => {}
            }
        }
    }
}
