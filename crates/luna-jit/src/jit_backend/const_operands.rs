//! The method JIT reads register operands only: a function's code is
//! rewritten with every constant- or immediate-operand opcode split into a
//! load and the register-operand opcode it abbreviates.

use luna_core::runtime::Value;
use luna_core::runtime::function::Proto;
use luna_core::vm::isa::{Inst, Op};

/// `proto.code` with the constant operands loaded into two registers
/// above the frame's own (`first_scratch` for integers, the next one for
/// floats, so neither is pinned to two kinds), and the jumps retargeted.
/// `None` when the split code cannot be expressed.
pub(super) fn split_const_operands(proto: &Proto, first_scratch: usize) -> Option<Vec<Inst>> {
    let code = &proto.code;
    if !code.iter().any(|&i| needs_split(i)) {
        return Some(code.to_vec());
    }
    // both scratch registers must fit an 8-bit operand
    if first_scratch + 1 > 255 {
        return None;
    }
    let (int_reg, float_reg) = (first_scratch as u32, first_scratch as u32 + 1);
    let mut out: Vec<Inst> = Vec::with_capacity(code.len() + 8);
    // old pc -> new pc, with one entry past the end
    let mut map: Vec<usize> = Vec::with_capacity(code.len() + 1);
    // new pc -> old pc of the instructions that carry a relative target
    let mut relative: Vec<(usize, usize)> = Vec::new();
    for (pc, &inst) in code.iter().enumerate() {
        map.push(out.len());
        match split(proto, inst, int_reg, float_reg) {
            Some(pair) => {
                // an instruction the one before it may skip must stay single
                if pc > 0 && skips_next(code[pc - 1].op()) {
                    return None;
                }
                out.extend(pair);
            }
            None => {
                if matches!(
                    inst.op(),
                    Op::Jmp | Op::ForPrep | Op::ForLoop | Op::TForPrep | Op::TForLoop
                ) {
                    relative.push((out.len(), pc));
                }
                out.push(inst);
            }
        }
    }
    map.push(out.len());
    for (new_pc, old_pc) in relative {
        let inst = out[new_pc];
        let at = |old_target: i64| -> Option<i64> {
            let t = usize::try_from(old_target).ok()?;
            map.get(t).map(|&n| n as i64)
        };
        let (old, new) = (old_pc as i64, new_pc as i64);
        out[new_pc] = match inst.op() {
            Op::Jmp => {
                let target = at(old + 1 + inst.sj() as i64)?;
                Inst::isj(Op::Jmp, i32::try_from(target - (new + 1)).ok()?)
            }
            // to the loop's `ForLoop`
            Op::ForPrep => {
                let target = at(old + inst.bx() as i64)?;
                Inst::iabx(Op::ForPrep, inst.a(), u32::try_from(target - new).ok()?)
            }
            // back to the loop body
            Op::ForLoop | Op::TForLoop => {
                let target = at(old + 1 - inst.bx() as i64)?;
                Inst::iabx(inst.op(), inst.a(), u32::try_from(new + 1 - target).ok()?)
            }
            // forward to the loop's `TForCall`
            _ => {
                let target = at(old + 1 + inst.bx() as i64)?;
                Inst::iabx(inst.op(), inst.a(), u32::try_from(target - (new + 1)).ok()?)
            }
        };
    }
    Some(out)
}

fn needs_split(inst: Inst) -> bool {
    inst.op() == Op::EqK || inst.split_const_operand(0).is_some()
}

/// The two instructions `inst` stands for, its operand loaded into the
/// scratch register of its kind.
fn split(proto: &Proto, inst: Inst, int_reg: u32, float_reg: u32) -> Option<[Inst; 2]> {
    let float_const = |k: u32| matches!(proto.consts.get(k as usize), Some(Value::Float(_)));
    if inst.op() == Op::EqK {
        let reg = if float_const(inst.b()) {
            float_reg
        } else {
            int_reg
        };
        return Some([
            Inst::iabx(Op::LoadK, reg, inst.b()),
            Inst::iabc(Op::Eq, inst.a(), reg, 0, inst.k()),
        ]);
    }
    let [load, _] = inst.split_const_operand(int_reg)?;
    let float = match load.op() {
        Op::LoadF => true,
        Op::LoadK => float_const(load.bx()),
        _ => false,
    };
    inst.split_const_operand(if float { float_reg } else { int_reg })
}

/// Opcodes that may skip the instruction after them.
fn skips_next(op: Op) -> bool {
    matches!(
        op,
        Op::Eq
            | Op::Lt
            | Op::Le
            | Op::EqK
            | Op::EqI
            | Op::LtI
            | Op::LeI
            | Op::GtI
            | Op::GeI
            | Op::Test
            | Op::TestSet
            | Op::LFalseSkip
    )
}
