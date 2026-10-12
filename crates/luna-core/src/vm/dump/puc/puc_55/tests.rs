use super::*;
use crate::vm::isa::Inst;

/// ivABC: `op:7 | A:8 | k:1 | vB:6 | vC:10`
fn vabck(op: u32, a: u32, vb: u32, vc: u32, k: bool) -> u32 {
    op | (a << 7) | ((k as u32) << 15) | (vb << 16) | (vc << 22)
}
fn ax(op: u32, ax: u32) -> u32 {
    op | (ax << 7)
}
const SETLIST: u32 = 78;
const EXTRAARG: u32 = 84;
const RETURN0: u32 = 71;

fn lower(code: Vec<u32>) -> Vec<Inst> {
    let mut heap = Heap::new();
    let mut raw = lower::test_proto(&mut heap, code, vec![], 8);
    translate(&mut raw).expect("translates").code
}

#[test]
fn setlist_extraarg_extends_the_ten_bit_offset() {
    // 3 values stored after 2 * 1024 + 5 elements
    let code = lower(vec![
        vabck(SETLIST, 0, 3, 5, true),
        ax(EXTRAARG, 2),
        vabck(RETURN0, 0, 0, 0, false),
    ]);
    assert!(code[0].k());
    assert_eq!(code[0].b(), 3);
    assert_eq!((code[1].op(), code[1].ax()), (Op::ExtraArg, 2 * 1024 + 5));
}
