use super::*;

/// `+` / `-` of two 5.1 / 5.2 integers, which stand for doubles: the
/// doubles' result is the exact one rounded once. Every such integer is a
/// double's exact value, so while the exact result is within ±2^53 it is
/// that double and stays an integer, as the interpreter keeps it
/// (`num_double::add`). A result past 2^53, which may round, or a machine
/// overflow leaves the trace at the op for the interpreter to do.
pub(super) fn emit_double_add<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) {
    const EXACT: i64 = 1 << 53;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    let lhs = lw.bcx.use_var(regs[ins.b() as usize]);
    let rhs = lw.bcx.use_var(regs[ins.c() as usize]);
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
    let shifted = lw.bcx.ins().iadd_imm_s(r, EXACT);
    let outside = lw
        .bcx
        .ins()
        .icmp_imm_u(IntCC::UnsignedGreaterThan, shifted, 2 * EXACT);
    let leave = lw.bcx.ins().bor(overflow, outside);
    let cont_blk = lw.bcx.create_block();
    let exit_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(leave, exit_blk, &[], cont_blk, &[]);
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    guard_exit(lw, pl, rop.pc, i);
    lw.bcx.switch_to_block(cont_blk);
    lw.bcx.seal_block(cont_blk);
    lw.bcx.def_var(regs[ins.a() as usize], r);
    lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
}
