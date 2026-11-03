//! The cases of `lir_primitives`: each emits one primitive (or one shape of
//! control flow) between loads of the inputs and stores of the results.

mod shapes;

use super::{Sigs, mem};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, BlockArg, MemFlagsData, StackSlotData, StackSlotKind, Type, Value, types,
};
use shapes::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum IntOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Ushr,
    Smin,
    Smax,
    Sdiv,
    Udiv,
    Umulhi,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ImmOp {
    AddU,
    AddS,
    AndU,
    AndS,
    XorU,
    ShlU,
    UshrU,
    SshrU,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum FloatOp {
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Floor,
    Ceil,
    ToSint,
    ToSintSat,
    Bits,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Case {
    Int(IntOp, Type),
    Neg(Type),
    Not(Type),
    Imm(ImmOp, i64),
    IcmpImm(IntCC, i64),
    Icmp(IntCC, Type),
    IcmpBranch(IntCC, Type),
    Select,
    Float(FloatOp),
    FloatUn(FloatOp),
    Fcmp(FloatCC),
    FcmpBranch(FloatCC),
    Reduce(Type),
    Extend(Type),
    FromSint(Type),
    IntToFloatBits,
    Memory(Type),
    Stack(Type),
    Uload8,
    Swap,
    Loop,
    Pressure,
    Calls,
    ManyArgs,
}

const NO: &[BlockArg] = &[];

/// Word `i` of the buffer.
fn at(i: i32) -> i32 {
    i * 8
}

fn out<E: Sigs>(e: &mut E, p: Value, i: i32, x: Value) {
    e.store(mem(), x, p, at(8 + i));
}

/// `cond ? 1 : 0` through a conditional branch and a block parameter.
fn branch_on<E: Sigs>(e: &mut E, cond: Value) -> Value {
    let (t, f, j) = (e.create_block(), e.create_block(), e.create_block());
    let r = e.append_block_param(j, types::I64);
    e.brif(cond, t, NO, f, NO);
    e.seal_block(t);
    e.seal_block(f);
    e.switch_to_block(t);
    let one = e.iconst(types::I64, 1);
    e.jump(j, &[BlockArg::Value(one)]);
    e.switch_to_block(f);
    let zero = e.iconst(types::I64, 0);
    e.jump(j, &[BlockArg::Value(zero)]);
    e.seal_block(j);
    e.switch_to_block(j);
    r
}

pub(super) fn emit<E: Sigs>(e: &mut E, case: &Case, p: Value) {
    let ld = |e: &mut E, ty: Type, i: i32| e.load(ty, mem(), p, at(i));
    match *case {
        Case::Int(op, ty) => {
            let (x, y) = (ld(e, ty, 0), ld(e, ty, 1));
            let r = match op {
                IntOp::Add => e.iadd(x, y),
                IntOp::Sub => e.isub(x, y),
                IntOp::Mul => e.imul(x, y),
                IntOp::And => e.band(x, y),
                IntOp::Or => e.bor(x, y),
                IntOp::Xor => e.bxor(x, y),
                IntOp::Shl => e.ishl(x, y),
                IntOp::Ushr => e.ushr(x, y),
                IntOp::Smin => e.smin(x, y),
                IntOp::Smax => e.smax(x, y),
                IntOp::Sdiv => e.sdiv(x, y),
                IntOp::Udiv => e.udiv(x, y),
                IntOp::Umulhi => e.umulhi(x, y),
            };
            out(e, p, 0, r);
        }
        Case::Neg(ty) | Case::Not(ty) => {
            let x = ld(e, ty, 0);
            let r = match case {
                Case::Neg(_) => e.ineg(x),
                _ => e.bnot(x),
            };
            out(e, p, 0, r);
        }
        Case::Imm(op, imm) => {
            let x = ld(e, types::I64, 0);
            let r = match op {
                ImmOp::AddU => e.iadd_imm_u(x, imm),
                ImmOp::AddS => e.iadd_imm_s(x, imm),
                ImmOp::AndU => e.band_imm_u(x, imm),
                ImmOp::AndS => e.band_imm_s(x, imm),
                ImmOp::XorU => e.bxor_imm_u(x, imm),
                ImmOp::ShlU => e.ishl_imm_u(x, imm),
                ImmOp::UshrU => e.ushr_imm_u(x, imm),
                ImmOp::SshrU => e.sshr_imm_u(x, imm),
            };
            out(e, p, 0, r);
        }
        Case::IcmpImm(cc, imm) => {
            let x = ld(e, types::I64, 0);
            let u = e.icmp_imm_u(cc, x, imm);
            let s = e.icmp_imm_s(cc, x, imm);
            out(e, p, 0, u);
            out(e, p, 1, s);
            let b = branch_on(e, s);
            out(e, p, 2, b);
        }
        Case::Icmp(cc, ty) => {
            let (x, y) = (ld(e, ty, 0), ld(e, ty, 1));
            let c = e.icmp(cc, x, y);
            out(e, p, 0, c);
            let w = e.uextend(types::I64, c);
            out(e, p, 1, w);
        }
        Case::IcmpBranch(cc, ty) => {
            let (x, y) = (ld(e, ty, 0), ld(e, ty, 1));
            let c = e.icmp(cc, x, y);
            let r = branch_on(e, c);
            out(e, p, 0, r);
        }
        Case::Select => {
            let (x, y) = (ld(e, types::I64, 0), ld(e, types::I64, 1));
            let c = e.icmp(IntCC::SignedLessThan, x, y);
            let r = e.select(c, x, y);
            out(e, p, 0, r);
            let five = e.iconst(types::I64, 5);
            let r = e.select(x, y, five);
            out(e, p, 1, r);
        }
        Case::Float(op) => {
            let (x, y) = (ld(e, types::F64, 0), ld(e, types::F64, 1));
            let r = match op {
                FloatOp::Add => e.fadd(x, y),
                FloatOp::Sub => e.fsub(x, y),
                FloatOp::Mul => e.fmul(x, y),
                FloatOp::Div => e.fdiv(x, y),
                other => unreachable!("{other:?} is unary"),
            };
            out(e, p, 0, r);
        }
        Case::FloatUn(op) => {
            let x = ld(e, types::F64, 0);
            let r = match op {
                FloatOp::Neg => e.fneg(x),
                FloatOp::Floor => e.floor(x),
                FloatOp::Ceil => e.ceil(x),
                FloatOp::ToSint => e.fcvt_to_sint(types::I64, x),
                FloatOp::ToSintSat => e.fcvt_to_sint_sat(types::I64, x),
                FloatOp::Bits => {
                    let b = e.bitcast(types::I64, MemFlagsData::new(), x);
                    e.iadd_imm_s(b, 1)
                }
                other => unreachable!("{other:?} is binary"),
            };
            out(e, p, 0, r);
        }
        Case::Fcmp(cc) => {
            let (x, y) = (ld(e, types::F64, 0), ld(e, types::F64, 1));
            let c = e.fcmp(cc, x, y);
            out(e, p, 0, c);
        }
        Case::FcmpBranch(cc) => {
            let (x, y) = (ld(e, types::F64, 0), ld(e, types::F64, 1));
            let c = e.fcmp(cc, x, y);
            let r = branch_on(e, c);
            out(e, p, 0, r);
        }
        Case::Reduce(ty) => {
            let x = ld(e, types::I64, 0);
            let r = e.ireduce(ty, x);
            out(e, p, 0, r);
            let w = e.uextend(types::I64, r);
            out(e, p, 1, w);
        }
        Case::Extend(ty) => {
            let x = ld(e, ty, 0);
            let w = e.uextend(types::I64, x);
            let s = e.iadd_imm_s(w, 1);
            out(e, p, 0, s);
        }
        Case::FromSint(ty) => {
            let x = ld(e, ty, 0);
            let r = e.fcvt_from_sint(types::F64, x);
            out(e, p, 0, r);
        }
        Case::IntToFloatBits => {
            let x = ld(e, types::I64, 0);
            let fx = e.bitcast(types::F64, MemFlagsData::new(), x);
            out(e, p, 0, fx);
            let d = e.fadd(fx, fx);
            out(e, p, 1, d);
        }
        Case::Memory(ty) => {
            let x = ld(e, ty, 0);
            // unaligned, and through a pointer at a negative offset
            e.store(mem(), x, p, at(8) + 3);
            let q = e.iadd_imm_s(p, at(10) as i64);
            let y = e.load(ty, mem(), q, -at(9) - 5);
            e.store(mem(), y, q, at(2));
        }
        Case::Stack(ty) => {
            let x = ld(e, ty, 0);
            let y = ld(e, ty, 1);
            let ss =
                e.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 24, 3));
            e.stack_store(types::I64, x, ss, 0);
            e.stack_store(types::I64, y, ss, 16);
            let a = e.stack_addr(types::I64, ss, 8);
            let back = e.load(ty, mem(), a, 8);
            out(e, p, 0, back);
            let back = e.stack_load(types::I64, ty, ss, 0);
            out(e, p, 1, back);
        }
        Case::Uload8 => {
            let a = e.uload8(types::I64, mem(), p, 1);
            out(e, p, 0, a);
            let b = e.uload8(types::I32, mem(), p, 7);
            out(e, p, 1, b);
        }
        Case::Swap => swap(e, p),
        Case::Loop => count_loop(e, p),
        Case::Pressure => {
            let (x, y) = (ld(e, types::I64, 0), ld(e, types::I64, 1));
            let r = pressure(e, x, y);
            out(e, p, 0, r);
        }
        Case::Calls => calls(e, p),
        Case::ManyArgs => many_args(e, p),
    }
    let r = ld(e, types::I64, 8);
    e.return_(&[r]);
}
