//! `Lir::calls_out`: what makes a trace's code run in the compiler's scope
//! when it runs from its parent's exit (`CompiledTraceParts::calls_helpers`).

use super::super::lir::{Inst, Lir, NONE, Op, Ty};
use cranelift_codegen::ir::Value;
use cranelift_codegen::packed_option::ReservedValue;

fn with(ops: &[Op]) -> Lir {
    let mut l = Lir::default();
    for &op in ops {
        l.insts.push(Inst {
            op,
            ty: Ty::I64,
            dst: NONE,
            a: 0,
            b: 0,
            c: 0,
            args_at: 0,
            n_args: 0,
            next: NONE,
            res: Value::reserved_value(),
        });
    }
    l
}

#[test]
fn each_kind_of_call_calls_out() {
    let plain = [Op::Jump, Op::VarRead, Op::VarWrite, Op::Return];
    assert!(!with(&plain).calls_out());
    for call in [Op::Call, Op::CallIndirect, Op::CallingExit(0)] {
        let mut ops = plain.to_vec();
        ops.insert(1, call);
        assert!(with(&ops).calls_out(), "{call:?}");
    }
}
