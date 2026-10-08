use super::*;

/// A bare function around `code`, for translator unit tests.
pub(in crate::vm::dump::puc) fn test_proto(
    heap: &mut Heap,
    code: Vec<u32>,
    consts: Vec<Value>,
    frame: u8,
) -> RawProto {
    RawProto {
        source: heap.intern(b"=test"),
        line_defined: 0,
        last_line_defined: 0,
        num_params: 0,
        is_vararg: false,
        has_compat_vararg_arg: false,
        vararg_table: false,
        max_stack: frame,
        code,
        consts,
        upvals: Vec::new(),
        protos: Vec::new(),
        lines: Vec::new(),
        locvars: Vec::new(),
    }
}

fn lowering(n: usize, frame: u8) -> Lowering {
    Lowering::new("test", n, frame, &[])
}

#[test]
fn a_puc_register_is_the_same_luna_register_and_scratch_goes_above_the_frame() {
    let mut lw = lowering(4, 10);
    assert_eq!(lw.r(7).unwrap(), 7);
    assert_eq!(lw.run(3, 3).unwrap(), 3);
    assert!(lw.r(256).is_err());
    lw.begin(0, 0);
    assert_eq!(lw.temp().unwrap(), 10);
}

#[test]
fn rk_operands_take_the_constant_forms() {
    let mut lw = lowering(4, 4);
    lw.begin(0, 0);
    lw.arith_rk(Op::Sub, 0, RK_BIT | 3, 1).unwrap();
    lw.arith_rk(Op::Div, 0, RK_BIT | 3, RK_BIT | 4).unwrap();
    lw.compare_rk(Op::Lt, true, RK_BIT | 2, 1).unwrap();
    lw.compare_rk(Op::Eq, false, RK_BIT | 2, RK_BIT | 5)
        .unwrap();
    let code = lw.finish(&[]).unwrap().code;
    let i = code[0];
    assert_eq!(
        (i.op(), i.a(), i.b(), i.c(), i.k()),
        (Op::SubK, 0, 1, 3, true)
    );
    let i = code[1];
    assert_eq!((i.op(), i.b(), i.c()), (Op::DivKK, 3, 4));
    let i = code[2];
    assert_eq!(
        (i.op(), i.a(), i.b(), i.c(), i.k()),
        (Op::LtK, 1, 2, 1, true)
    );
    let i = code[3];
    assert_eq!((i.op(), i.a(), i.b(), i.k()), (Op::EqKK, 2, 5, false));
}

#[test]
fn a_local_takes_the_register_after_the_locals_live_at_its_start() {
    let raw = [
        ("a", 0, 10),
        ("b", 2, 5),
        ("c", 6, 10), // b is gone by now: c reuses its register
        ("d", 7, 9),
    ]
    .map(|(n, s, e)| RawLocVar {
        name: n.into(),
        start_pc: s,
        end_pc: e,
    });
    let mut lw = lowering(10, 10);
    for pc in 0..10 {
        lw.begin(pc, 0);
        lw.emit(Inst::iabc(Op::Move, 0, 0, 0, false));
    }
    let regs: Vec<u32> = lw
        .finish(&raw)
        .unwrap()
        .locvars
        .iter()
        .map(|v| v.reg)
        .collect();
    assert_eq!(regs, [0, 1, 1, 2]);
}

#[test]
fn a_guarded_closing_jump_stays_one_instruction() {
    let mut lw = lowering(3, 4);
    lw.begin(0, 0);
    lw.emit(Inst::iabc(Op::Test, 0, 0, 0, true));
    lw.begin(1, 0);
    lw.jump_closing(2, 0).unwrap();
    lw.begin(2, 0);
    lw.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
    let code = lw.finish(&[]).unwrap().code;
    assert_eq!(code.len(), 5);
    assert_eq!(code[1].op(), Op::Jmp);
    assert_eq!(
        1 + 1 + code[1].sj(),
        3,
        "the test's jump goes to the trampoline"
    );
    assert_eq!((code[3].op(), code[3].a()), (Op::Close, 2));
    assert_eq!(code[4].op(), Op::Jmp);
    assert_eq!(4 + 1 + code[4].sj(), 0, "the trampoline goes to the target");
}

#[test]
fn a_constant_index_past_bx_loads_through_extraarg() {
    let mut lw = lowering(1, 2);
    lw.begin(0, 0);
    lw.load_k(1, isa::MAX_BX + 1).unwrap();
    let code = lw.finish(&[]).unwrap().code;
    assert_eq!(code[0].op(), Op::LoadKx);
    assert_eq!(
        (code[1].op(), code[1].ax()),
        (Op::ExtraArg, isa::MAX_BX + 1)
    );
}

#[test]
fn concat_into_another_register_names_it() {
    let mut lw = lowering(1, 8);
    lw.begin(0, 0);
    lw.concat_range(5, 2, 4).unwrap();
    let code = lw.finish(&[]).unwrap().code;
    assert_eq!(code.len(), 1);
    let i = code[0];
    assert_eq!(
        (i.op(), i.a(), i.b(), i.c(), i.k()),
        (Op::Concat, 5, 3, 2, true)
    );
}

#[test]
fn set_list_offsets_past_c_use_extraarg() {
    let mut lw = lowering(1, 8);
    lw.begin(0, 0);
    lw.set_list(1, 3, 300).unwrap();
    let code = lw.finish(&[]).unwrap().code;
    assert!(code[0].k());
    assert_eq!((code[1].op(), code[1].ax()), (Op::ExtraArg, 300));
}

#[test]
fn abslineinfo_sets_the_line_instead_of_adding() {
    // pc0 +1, pc1 absolute 500, pc2 +2
    let lines = rle_lines("t", &[1, 0x80, 2], &[(1, 500)], 10, 3).unwrap();
    assert_eq!(lines, vec![11, 500, 502]);
}
