//! Arithmetic on two 5.1 / 5.2 integers, which stand for doubles. Every
//! such integer is a double's exact value, so while an exact result is
//! within ±2^53 it is the double the operation gives, and the interpreter
//! keeps it as an integer too (`num_double`). Where the doubles would
//! round, overflow or give -0, the trace leaves at the op and the
//! interpreter does it.

use super::*;

const EXACT: i64 = 1 << 53;

/// Leaves the trace at the op when `leave` is set.
fn leave_if<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>, leave: Value) {
    let cont_blk = lw.bcx.create_block();
    let exit_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(leave, exit_blk, &[], cont_blk, &[]);
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    guard_exit(lw, pl, oc.rop.pc, oc.i);
    lw.bcx.switch_to_block(cont_blk);
    lw.bcx.seal_block(cont_blk);
}

/// Whether `v` is outside -2^53 ..= 2^53.
fn outside_exact<E: Emit>(lw: &mut Lower<E>, v: Value) -> Value {
    let shifted = lw.bcx.ins().iadd_imm_s(v, EXACT);
    lw.bcx
        .ins()
        .icmp_imm_u(IntCC::UnsignedGreaterThan, shifted, 2 * EXACT)
}

fn define_int<E: Emit>(lw: &mut Lower<E>, oc: &OpCx<'_>, r: Value) {
    lw.bcx.def_var(oc.regs[oc.ins.a() as usize], r);
    lw.current_kinds[oc.off + oc.ins.a() as usize] = RegKind::Int;
}

/// `+` / `-`: the exact sum, unless it is past ±2^53 (it may round) or
/// the machine addition overflows.
pub(super) fn emit_double_add<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) {
    let ins = oc.ins;
    let lhs = lw.bcx.use_var(oc.regs[ins.b() as usize]);
    let rhs = lw.bcx.use_var(oc.regs[ins.c() as usize]);
    let (r, overflow) = if oc.op == Op::Add {
        let r = lw.bcx.ins().iadd(lhs, rhs);
        // the operands share a sign that the result lacks
        let x = lw.bcx.ins().bxor(lhs, r);
        let y = lw.bcx.ins().bxor(rhs, r);
        (r, lw.bcx.ins().band(x, y))
    } else {
        let r = lw.bcx.ins().isub(lhs, rhs);
        // the operands' signs differ and the result's is not the left one's
        let x = lw.bcx.ins().bxor(lhs, rhs);
        let y = lw.bcx.ins().bxor(lhs, r);
        (r, lw.bcx.ins().band(x, y))
    };
    let overflow = lw.bcx.ins().icmp_imm_s(IntCC::SignedLessThan, overflow, 0);
    let outside = outside_exact(lw, r);
    let leave = lw.bcx.ins().bor(overflow, outside);
    leave_if(lw, pl, oc, leave);
    define_int(lw, oc, r);
}

/// `*`: the exact product, unless it is past ±2^53 or it is zero with a
/// negative operand (the doubles give -0). The product of the operands as
/// floats is within a hair of the exact one, so when it is within ±2^54
/// the machine product cannot overflow.
pub(super) fn emit_double_mul<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) {
    let ins = oc.ins;
    let lhs = lw.bcx.use_var(oc.regs[ins.b() as usize]);
    let rhs = lw.bcx.use_var(oc.regs[ins.c() as usize]);
    let fl = lw.bcx.ins().fcvt_from_sint(types::F64, lhs);
    let fr = lw.bcx.ins().fcvt_from_sint(types::F64, rhs);
    let fp = lw.bcx.ins().fmul(fl, fr);
    let hi = lw.bcx.ins().f64const((2 * EXACT) as f64);
    let lo = lw.bcx.ins().f64const(-(2 * EXACT) as f64);
    let above = lw.bcx.ins().fcmp(FloatCC::GreaterThan, fp, hi);
    let below = lw.bcx.ins().fcmp(FloatCC::LessThan, fp, lo);
    let far = lw.bcx.ins().bor(above, below);
    leave_if(lw, pl, oc, far);
    let r = lw.bcx.ins().imul(lhs, rhs);
    let outside = outside_exact(lw, r);
    let zero = lw.bcx.ins().icmp_imm_s(IntCC::Equal, r, 0);
    let signs = lw.bcx.ins().bor(lhs, rhs);
    let negative = lw.bcx.ins().icmp_imm_s(IntCC::SignedLessThan, signs, 0);
    let minus_zero = lw.bcx.ins().band(zero, negative);
    let leave = lw.bcx.ins().bor(outside, minus_zero);
    leave_if(lw, pl, oc, leave);
    define_int(lw, oc, r);
}

/// `%`: `a - floor(a/b)*b`, which for operands within ±2^53 and a nonzero
/// divisor is the integer floor modulo. Anything else (a zero divisor
/// gives nan) is the interpreter's. A constant divisor is checked here.
pub(super) fn emit_double_mod<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let ins = oc.ins;
    let lhs = lw.bcx.use_var(oc.regs[ins.b() as usize]);
    let outside = outside_exact(lw, lhs);
    let r = match oc.rc_const {
        Some(k) if k != 0 && k != -1 && k.unsigned_abs() <= EXACT as u64 => {
            leave_if(lw, pl, oc, outside);
            emit_floor_divmod_by(&mut lw.bcx, Op::Mod, lhs, k)
        }
        Some(_) => return None,
        None => {
            let rhs = lw.bcx.use_var(oc.regs[ins.c() as usize]);
            let zero = lw.bcx.ins().icmp_imm_s(IntCC::Equal, rhs, 0);
            let rhs_outside = outside_exact(lw, rhs);
            let leave = lw.bcx.ins().bor(outside, rhs_outside);
            let leave = lw.bcx.ins().bor(leave, zero);
            leave_if(lw, pl, oc, leave);
            emit_floor_divmod(&mut lw.bcx, Op::Mod, lhs, rhs)
        }
    };
    define_int(lw, oc, r);
    Some(())
}

/// Unary `-`: the negated integer, unless it is zero (the doubles give -0)
/// or the smallest integer (its negation is not an integer).
pub(super) fn emit_double_neg<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) {
    let src = lw.bcx.use_var(oc.regs[oc.ins.b() as usize]);
    let zero = lw.bcx.ins().icmp_imm_s(IntCC::Equal, src, 0);
    let min = lw.bcx.ins().icmp_imm_s(IntCC::Equal, src, i64::MIN);
    let leave = lw.bcx.ins().bor(zero, min);
    leave_if(lw, pl, oc, leave);
    let r = lw.bcx.ins().ineg(src);
    define_int(lw, oc, r);
}
