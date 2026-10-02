use super::*;

/// Ops lowered through a helper or as plain moves and loads.
pub(super) fn validate_body_op(
    max_stack: usize,
    rop: &RecordedOp,
    op: Op,
    ins: Inst,
    a: usize,
    b: usize,
    c: usize,
) -> Option<()> {
    match op {
        Op::TForPrep => {
            // generic-for prep: forward `add_pc(bx)`
            // to the body-tail (TForCall). Recorder enters at
            // body_top = head_pc, AFTER TForPrep, so the record
            // body never sees TForPrep in normal pickup; bail if
            // it shows up (mid-body / inline-depth>0 = unsupported
            // shape).
            {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
            }
        }
        Op::TForCall => {
            // generic-for body tail. Calls iter
            // via the `luna_jit_op_tforcall` helper. Bounds:
            // helper accesses R[A..A+7] (gen/state/ctrl plus the
            // generator-call window R[A+4..A+6] + space for the
            // first two returns). Restrict to inline_depth = 0
            // (helper reads vm.stack via the trace head's frame
            // base; inline frames aren't pushed during trace IR
            // execution). C field = nvars in [1, 250) per PUC.
            if rop.inline_depth > 0 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if a + 6 >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let nvars = ins.c();
            if nvars == 0 || nvars > 250 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::Concat => {
            // N-operand right-associative concat.
            // Helper reads vm.stack[base+A..base+A+B); spill all
            // operand slots in body emit. Restrict to depth=0
            // (helper resolves base via trace head's Lua frame).
            if rop.inline_depth > 0 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let n_operands = ins.b() as usize;
            if n_operands < 2 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            match a.checked_add(n_operands) {
                Some(end) if end <= max_stack => {}
                _ => return None,
            }
        }
        Op::GetTabUp => {
            // standalone GetTabUp body bounds.
            // K[C] = Str validated by the upstream cmp-dirs gate;
            // here we just bounds-check the A register.
            if a >= max_stack {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
            }
        }
        Op::SetField | Op::GetField => {
            // validated above (Str const at K[B] or
            // K[C] respectively); bounds-check the reg operands
            // here.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if matches!(op, Op::SetField) && c >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if matches!(op, Op::GetField) && b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::Jmp => {
            // Validated in the second pass below.
        }
        Op::Move => {
            if a >= max_stack || b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::LoadI => {
            // R[A] := signed-bx immediate. No reg operand
            // beyond A; sBx fits in i32 (decoded from u32 by
            // Inst::sbx) so the i64 conversion is lossless.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::LoadF => {
            // R[A] := signed-bx immediate as f64.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::LoadNil => {
            // R[A..=A+B] := nil. Validate the full
            // range fits in window; emit pass writes iconst(0)
            // per slot.
            match a.checked_add(b) {
                Some(end) if end < max_stack => {}
                _ => return None,
            }
        }
        Op::Close => {
            // close open upvals at slot ≥ A.
            // Limited to inline_depth=0 (helper reads vm.stack
            // via the trace-head frame's base; inline frames aren't
            // pushed). Bounds check on A.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if rop.inline_depth > 0 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        _ => unreachable!("routed by validate_op"),
    }
    Some(())
}

/// Closures, constants, arithmetic and `EqK`.
pub(super) fn validate_value_op(
    record: &TraceRecord,
    vconsts: &[Option<VConst>],
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
    let oob = |i: usize, r: u32| {
        r as usize >= max_stack && !(r as usize == max_stack && vconst_at(vconsts, i).is_some())
    };
    match op {
        Op::Closure => {
            // R[A] := closure(proto.protos[Bx]).
            // Shared-upval / 0-upval closures, plus in_stack
            // upval support via per-upval pre-Closure
            // spill (emit writes vm.stack[base + d.index] from
            // regs[d.index] before calling op_closure helper).
            //
            // Restrictions:
            // - depth = 0 only: spill writes vm.stack via the
            //   trace-head frame's `base`; inline frames (depth>0)
            //   aren't pushed during trace IR execution, so a
            //   spill at depth>0 would target wrong slots.
            // - Source slot must have a known RegKind (not Unset):
            //   spill needs a tag to pack the i64 payload back to
            //   a Value. Unset would mean trace never wrote the
            //   slot AND entry_tags didn't snapshot it.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if rop.inline_depth > 0 {
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
            // R[A] := proto.consts[Bx]. Step-8 only lowers
            // Int / Float consts; Str / Bool / Nil need a
            // wider marshalling story.
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
                luna_core::runtime::Value::Int(_) | luna_core::runtime::Value::Float(_)
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
            if !matches!(
                head_proto.consts[bx],
                luna_core::runtime::Value::Int(_) | luna_core::runtime::Value::Float(_)
            ) {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
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
            if !matches!(next.inst.op(), Op::Jmp) || next.pc != rop.pc + 1 {
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
