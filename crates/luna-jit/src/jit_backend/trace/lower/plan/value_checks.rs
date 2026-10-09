//! Ops that read or write values: the checks before lowering them.

use super::*;

/// Closures, constants, arithmetic and `EqK`.
pub(super) fn validate_value_op(
    record: &TraceRecord,
    vconsts: &[VRegs],
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    i: usize,
    rop: &RecordedOp,
    op: Op,
    ins: Inst,
    a: usize,
    b: usize,
    c: usize,
    consumed_by_cmp: &mut [bool],
) -> Option<()> {
    let oob =
        |i: usize, r: u32| r as usize >= max_stack && virt_at(vconsts, i, r, max_stack).is_none();
    match op {
        Op::Closure => {
            // R[A] := closure(proto.protos[Bx]).
            // Shared-upval / 0-upval closures, plus in_stack
            // upval support via per-upval pre-Closure
            // spill (emit writes vm.stack[base + d.index] from
            // regs[d.index] before calling op_closure helper).
            //
            // The spill and the helper address the frame's slots from
            // the head frame, inlined frames included. A source slot
            // needs a known kind (the emit gives up otherwise): the
            // spill packs its payload with that tag.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let bx = ins.bx() as usize;
            if bx >= head_proto.protos.len() {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let inner = head_proto.protos[bx];
            for d in inner.upvals.iter() {
                if !d.in_stack {
                    continue;
                }
                let src_idx = d.index as usize;
                if src_idx >= max_stack {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
            }
        }
        Op::LoadK => {
            // R[A] := proto.consts[Bx]: a number or a string (its
            // pointer, the constant table keeping it alive)
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let bx = ins.bx() as usize;
            if bx >= head_proto.consts.len() {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if !matches!(
                head_proto.consts[bx],
                luna_core::runtime::Value::Int(_)
                    | luna_core::runtime::Value::Float(_)
                    | luna_core::runtime::Value::Str(_)
            ) {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => {
            if a >= max_stack || oob(i, b as u32) || oob(i, c as u32) {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        // 3-reg Int arith / bitwise ops — same bounds rules as
        // Add/Sub/Mul. Operand-type assumed Int (recorder is
        // trusted); Float / mixed paths would need RegKind
        // tracking like the method JIT.
        Op::IDiv | Op::Mod | Op::BAnd | Op::BOr | Op::BXor | Op::Shl | Op::Shr => {
            if a >= max_stack || oob(i, b as u32) || oob(i, c as u32) {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        // 2-reg unary: `R[A] := op R[B]` — Unm (negation),
        // BNot (bitwise NOT).
        Op::Unm | Op::BNot => {
            if a >= max_stack || b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        // `if (R[A] == const[B]) ~= K then pc++` — same
        // cmp-then-Jmp shape as Lt/Le/Eq. Const RHS is either
        // an Int (icmp eq) or a Float (fcmp eq).
        Op::EqK => {
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let bx = ins.b() as usize;
            if bx >= head_proto.consts.len() {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            // a short string is interned: equal ones are one object
            let comparable = match head_proto.consts[bx] {
                luna_core::runtime::Value::Int(_)
                | luna_core::runtime::Value::Float(_)
                | luna_core::runtime::Value::Bool(_) => true,
                luna_core::runtime::Value::Str(k) => {
                    k.len() <= luna_core::runtime::string::MAX_SHORT_LEN
                }
                _ => false,
            };
            if !comparable {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
            }
            // EqK pairs with the same trailing Jmp at
            // cmp_pc + 1 contract as Lt/Le/Eq.
            if i + 1 >= effective_end {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let next = &record.ops[i + 1];
            if !next.inst.op().is_jump() || next.pc != rop.pc + 1 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            consumed_by_cmp[i + 1] = true;
        }
        _ => unreachable!("routed by validate_op"),
    }
    Some(())
}
