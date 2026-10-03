//! The cases of `lir_primitives`: each emits one primitive (or one shape of
//! control flow) between loads of the inputs and stores of the results.

use super::{Sigs, mem};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, BlockArg, MemFlagsData, StackSlotData, StackSlotKind, Type, Value, types,
};

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

/// Block arguments that rotate: the moves into the parameters form a cycle.
fn swap<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let z = e.load(types::I64, mem(), p, at(2));
    let (j1, j2) = (e.create_block(), e.create_block());
    let ps1: Vec<Value> = (0..3)
        .map(|_| e.append_block_param(j1, types::I64))
        .collect();
    let ps2: Vec<Value> = (0..3)
        .map(|_| e.append_block_param(j2, types::I64))
        .collect();
    let args = |vs: &[Value]| -> Vec<BlockArg> { vs.iter().map(|&v| BlockArg::Value(v)).collect() };
    e.jump(j1, &args(&[y, z, x]));
    e.seal_block(j1);
    e.switch_to_block(j1);
    e.jump(j2, &args(&[ps1[1], ps1[2], ps1[0]]));
    e.seal_block(j2);
    e.switch_to_block(j2);
    let t = e.ishl_imm_u(ps2[1], 1);
    let r = e.isub(ps2[0], t);
    let r = e.bxor(r, ps2[2]);
    out(e, p, 0, r);
    out(e, p, 1, ps2[2]);
}

/// A loop over variables: the header reads them before its back edge is
/// known.
fn count_loop<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let n = e.load(types::I64, mem(), p, at(2));
    let (i, acc) = (e.declare_var(types::I64), e.declare_var(types::I64));
    let zero = e.iconst(types::I64, 0);
    e.def_var(i, zero);
    e.def_var(acc, x);
    let (head, body, exit) = (e.create_block(), e.create_block(), e.create_block());
    e.jump(head, NO);
    e.switch_to_block(head);
    let iv = e.use_var(i);
    let c = e.icmp(IntCC::SignedLessThan, iv, n);
    e.brif(c, body, NO, exit, NO);
    e.seal_block(body);
    e.switch_to_block(body);
    let a = e.use_var(acc);
    let iv = e.use_var(i);
    let k = e.iconst(types::I64, 31);
    let m = e.imul(a, k);
    let t = e.bxor(iv, x);
    let a2 = e.iadd(m, t);
    e.def_var(acc, a2);
    let i2 = e.iadd_imm_s(iv, 1);
    e.def_var(i, i2);
    e.jump(head, NO);
    e.seal_block(head);
    e.seal_block(exit);
    e.switch_to_block(exit);
    let a = e.use_var(acc);
    let iv = e.use_var(i);
    out(e, p, 0, a);
    out(e, p, 1, iv);
}

/// More values live at once than either target has registers.
fn pressure<E: Sigs>(e: &mut E, x: Value, y: Value) -> Value {
    let ints: Vec<Value> = (1..=40i64)
        .map(|k| {
            let kc = e.iconst(types::I64, k);
            let m = e.imul(x, kc);
            let a = e.iadd_imm_s(y, k);
            e.bxor(m, a)
        })
        .collect();
    let fx = e.fcvt_from_sint(types::F64, x);
    let floats: Vec<Value> = (1..=20)
        .map(|k| {
            let c = e.f64const(k as f64 * 0.75);
            e.fmul(fx, c)
        })
        .collect();
    let mut acc = e.iconst(types::I64, 0);
    for (k, &v) in ints.iter().enumerate().rev() {
        acc = if k % 2 == 0 {
            e.isub(acc, v)
        } else {
            e.bxor(acc, v)
        };
    }
    let mut facc = e.f64const(0.5);
    for &v in floats.iter().rev() {
        facc = e.fsub(v, facc);
    }
    let bits = e.bitcast(types::I64, MemFlagsData::new(), facc);
    e.iadd(acc, bits)
}

extern "C" fn mix(a: i64, b: f64, c: i64, d: f64) -> i64 {
    a.wrapping_mul(3) ^ (b.to_bits() as i64) ^ c.rotate_left(7) ^ (d.to_bits() as i64 >> 3)
}

extern "C" fn fmix(a: f64, b: f64) -> f64 {
    a * 2.0 - b
}

/// Indirect calls with integer and float arguments, values live across them.
fn calls<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let z = e.load(types::I64, mem(), p, at(2));
    let live: Vec<Value> = (1..=16i64).map(|k| e.iadd_imm_s(x, k * 1000)).collect();
    let fy = e.fcvt_from_sint(types::F64, y);
    let mut s = e.make_sig();
    for t in [types::I64, types::F64, types::I64, types::F64] {
        s.params.push(AbiParam::new(t));
    }
    s.returns.push(AbiParam::new(types::I64));
    let s = e.sig(s);
    let mut fs = e.make_sig();
    fs.params.push(AbiParam::new(types::F64));
    fs.params.push(AbiParam::new(types::F64));
    fs.returns.push(AbiParam::new(types::F64));
    let fs = e.sig(fs);
    let callee = e.iconst(types::I64, mix as *const () as usize as i64);
    let two = e.f64const(2.5);
    let call = e.call_indirect(s, callee, &[x, fy, z, two]);
    let r1 = e.inst_results(call)[0];
    let callee = e.iconst(types::I64, fmix as *const () as usize as i64);
    let call = e.call_indirect(fs, callee, &[fy, two]);
    let r2 = e.inst_results(call)[0];
    let mut acc = r1;
    for &v in &live {
        acc = e.isub(acc, v);
        acc = e.bxor(acc, y);
    }
    out(e, p, 0, acc);
    out(e, p, 1, r2);
    out(e, p, 2, fy);
}

#[allow(clippy::too_many_arguments)]
extern "C" fn wide(
    a: i64,
    f: f64,
    b: i64,
    g: f64,
    c: i64,
    h: f64,
    d: i64,
    i: f64,
    e: i64,
    j: f64,
    k: i64,
    l: f64,
    m: i64,
    n: f64,
    o: i64,
    q: f64,
    r: i64,
) -> i64 {
    let ints = [a, b, c, d, e, k, m, o, r];
    let floats = [f, g, h, i, j, l, n, q];
    let mut acc = 0i64;
    for (s, x) in ints.iter().enumerate() {
        acc = acc.wrapping_mul(31).wrapping_add(x ^ s as i64);
    }
    for x in floats {
        acc = acc.wrapping_mul(17) ^ x.to_bits() as i64;
    }
    acc
}

/// More arguments than either target passes in registers, integers and
/// floats interleaved: the rest go on the stack in each ABI's order.
fn many_args<E: Sigs>(e: &mut E, p: Value) {
    let x = e.load(types::I64, mem(), p, at(0));
    let y = e.load(types::I64, mem(), p, at(1));
    let mut s = e.make_sig();
    let mut args = Vec::new();
    for k in 0..17i64 {
        if k % 2 == 1 && k < 16 {
            s.params.push(AbiParam::new(types::F64));
            let c = e.iconst(types::I64, k);
            let v = e.iadd(y, c);
            args.push(e.fcvt_from_sint(types::F64, v));
        } else {
            s.params.push(AbiParam::new(types::I64));
            let c = e.iconst(types::I64, k * 1000);
            args.push(e.bxor(x, c));
        }
    }
    s.returns.push(AbiParam::new(types::I64));
    let s = e.sig(s);
    let callee = e.iconst(types::I64, wide as *const () as usize as i64);
    let call = e.call_indirect(s, callee, &args);
    let r = e.inst_results(call)[0];
    out(e, p, 0, r);
    // values live across the call come back intact
    let after = e.iadd(args[0], args[16]);
    out(e, p, 1, after);
}
