use super::*;

/// Jumps, moves and loads.
pub(super) fn emit_basic_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan { head_proto, .. } = *pl;
    let OpCx { off, ins, .. } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::Jmp => {
            // Trailing back-edge (validated in the pre-emit
            // pass). The tail's `return iconst(head_pc)`
            // carries the control transfer.
        }
        Op::Move => {
            let src = lw.bcx.use_var(regs[ins.b() as usize]);
            lw.bcx.def_var(regs[ins.a() as usize], src);
            lw.current_kinds[off + ins.a() as usize] =
                k_op(&lw.current_kinds, off as u32 + ins.b());
        }
        Op::LoadI => {
            let imm = ins.sbx() as i64;
            let v = lw.bcx.ins().iconst(types::I64, imm);
            lw.bcx.def_var(regs[ins.a() as usize], v);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Int;
            lw.known_int[off + ins.a() as usize] = Some(imm);
        }
        Op::LoadF => {
            // R[A] := sBx as f64. Bitcast result to i64
            // bit-pattern so the reg's storage stays uniform.
            let f = ins.sbx() as f64;
            let v = lw.bcx.ins().f64const(f);
            def_var_f64(&mut lw.bcx, regs[ins.a() as usize], v);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Float;
        }
        Op::LoadNil => {
            // R[A..=A+B] := nil. NIL raw payload bits
            // are 0; emit one iconst(0) and def_var it into each
            // target slot, marking current_kinds = Nil so the
            // exit-tag derivation (kinds_to_exit_tags)
            // produces ExitTag::Nil for slots the trace touched.
            let a_us = ins.a() as usize;
            let b_us = ins.b() as usize;
            let zero = lw.bcx.ins().iconst(types::I64, 0);
            for k in 0..=b_us {
                lw.bcx.def_var(regs[a_us + k], zero);
                lw.current_kinds[off + a_us + k] = RegKind::Nil;
            }
        }
        Op::LoadK => {
            let bx = ins.bx() as usize;
            let (v, k) = match head_proto.consts[bx] {
                luna_core::runtime::Value::Int(n) => {
                    lw.known_int[off + ins.a() as usize] = Some(n);
                    (lw.bcx.ins().iconst(types::I64, n), RegKind::Int)
                }
                luna_core::runtime::Value::Float(f) => {
                    let fv = lw.bcx.ins().f64const(f);
                    let bits = lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), fv);
                    (bits, RegKind::Float)
                }
                luna_core::runtime::Value::Str(k) => (
                    emit_str_key_arg(
                        lw.module,
                        &mut lw.bcx,
                        k,
                        pl.opts.aot,
                        &mut lw.defined_aot_data,
                    ),
                    RegKind::Str,
                ),
                _ => unreachable!("pre-emit gates number and string consts"),
            };
            lw.bcx.def_var(regs[ins.a() as usize], v);
            lw.current_kinds[off + ins.a() as usize] = k;
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}
