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
/// entry block.
pub(super) fn hoist(
    r: &mut Replay<'_, '_>,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
) {
    use crate::jit_backend::trace::RelocKind;
    let keep = relocs
        .iter()
        .enumerate()
        .filter(|(_, (k, _))| matches!(k, RelocKind::Str | RelocKind::Proto))
        .map(|(n, _)| n)
        .take(HOISTED);
    for n in keep {
        let v = r.b.ins().symbol_value(types::I64, r.reloc_gv[n]);
        r.hoisted[n] = Some(v);
    }
}
