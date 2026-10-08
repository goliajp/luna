use super::*;

/// Ops lowered through a helper or as plain moves and loads.
pub(super) fn validate_body_op(
    vconsts: &[VRegs],
    i: usize,
    max_stack: usize,
    rop: &RecordedOp,
    op: Op,
    ins: Inst,
    a: usize,
    b: usize,
    c: usize,
) -> Option<()> {
    match op {
        Op::TForPrep | Op::TForPrep53 | Op::TForPrep55 => {
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
        Op::TForCall | Op::TForCall53 | Op::TForCall55 => {
            // generic-for body tail. Calls iter
            // via the `luna_jit_op_tforcall` helper. Bounds: the
            // helper grows the stack to R[A+6] for the generator-call
            // window R[A+4..A+6] itself; the trace holds R[A..A+5],
            // R[A+5] only when the frame has it. Restrict to inline_depth = 0
            // (helper reads vm.stack via the trace head's frame
            // base; inline frames aren't pushed during trace IR
            // execution). C field = nvars in [1, 250) per PUC.
            if rop.inline_depth > 0 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if a + op.for_layout()?.var() as usize >= max_stack {
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
            // validated above (Str const at K[B] or K[C] respectively);
            // the table and the stored value may be the op's virtual
            // registers
            let oob =
                |r: usize| r >= max_stack && virt_at(vconsts, i, r as u32, max_stack).is_none();
            let bad = match op {
                Op::SetField => oob(a) || oob(c),
                _ => a >= max_stack || oob(b),
            };
            if bad {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
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
        Op::LoadFalse | Op::LoadTrue | Op::LFalseSkip => {
            if a >= max_stack {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
            }
        }
        Op::Not => {
            if a >= max_stack || b >= max_stack {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
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
        Op::Close | Op::JmpClose | Op::JmpCloseBack => {
            // close open upvals at slot ≥ A of the op's frame (the
            // helper counts slots from the head frame, inlined frames
            // included); a closing jump closes from A - 1
            let a = if op == Op::Close { a } else { a - 1 };
            if a >= max_stack {
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
