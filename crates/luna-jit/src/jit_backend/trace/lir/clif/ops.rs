//! The arithmetic of [`Replay`] and the relocations it reads in the entry
//! block.

use super::*;

impl Replay<'_, '_> {
    pub(super) fn bin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        let ins = self.b.ins();
        match op {
            BinOp::Add => ins.iadd(x, y),
            BinOp::Sub => ins.isub(x, y),
            BinOp::Mul => ins.imul(x, y),
            BinOp::Sdiv => ins.sdiv(x, y),
            BinOp::Umulhi => ins.umulhi(x, y),
            BinOp::Smin => ins.smin(x, y),
            BinOp::Smax => ins.smax(x, y),
            BinOp::And => ins.band(x, y),
            BinOp::Or => ins.bor(x, y),
            BinOp::Xor => ins.bxor(x, y),
            BinOp::Shl => ins.ishl(x, y),
            BinOp::Ushr => ins.ushr(x, y),
            BinOp::Sshr => ins.sshr(x, y),
            BinOp::Fadd => ins.fadd(x, y),
            BinOp::Fsub => ins.fsub(x, y),
            BinOp::Fmul => ins.fmul(x, y),
            BinOp::Fdiv => ins.fdiv(x, y),
        }
    }

    pub(super) fn un(&mut self, u: UnOp, i: &Inst) -> Value {
        let x = self.v(i.a);
        let from = self.lir.value_ty[i.a as usize];
        let t = cty(i.ty);
        let ins = self.b.ins();
        match u {
            UnOp::Ineg => ins.ineg(x),
            UnOp::Bnot => ins.bnot(x),
            UnOp::Fneg => ins.fneg(x),
            UnOp::Floor => ins.floor(x),
            UnOp::Ceil => ins.ceil(x),
            UnOp::Uextend if from == i.ty => x,
            UnOp::Uextend => ins.uextend(t, x),
            UnOp::Ireduce if from == i.ty => x,
            UnOp::Ireduce => ins.ireduce(t, x),
            UnOp::Bitcast => ins.bitcast(t, MemFlagsData::new(), x),
            UnOp::FcvtFromSint => ins.fcvt_from_sint(t, x),
            UnOp::FcvtToSint => ins.fcvt_to_sint(t, x),
            UnOp::FcvtToSintSat => ins.fcvt_to_sint_sat(t, x),
        }
    }
}

/// Reads the relocations [`Replay::hoisted`] keeps, at the start of the
/// entry block: string and function ones read at least twice inside a
/// loop, the most read first, at most [`HOISTED`]. Each of those saves a
/// read per iteration; one read once would only hold a register.
pub(super) fn hoist(
    r: &mut Replay<'_, '_>,
    an: &live::Analysis,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
) {
    use crate::jit_backend::trace::RelocKind;
    let mut in_loops = vec![0u32; relocs.len()];
    for &(head, back) in &an.loops {
        for c in head / 2..=back / 2 {
            if let Op::Reloc(n) = r.lir.insts[an.code[c as usize] as usize].op {
                in_loops[n as usize] += 1;
            }
        }
    }
    let mut keep: Vec<usize> = (0..relocs.len())
        .filter(|&n| in_loops[n] >= 2 && matches!(relocs[n].0, RelocKind::Str | RelocKind::Proto))
        .collect();
    keep.sort_by_key(|&n| std::cmp::Reverse(in_loops[n]));
    for &n in keep.iter().take(HOISTED) {
        let v = r.b.ins().symbol_value(types::I64, r.reloc_gv[n]);
        r.hoisted[n] = Some(v);
    }
}
