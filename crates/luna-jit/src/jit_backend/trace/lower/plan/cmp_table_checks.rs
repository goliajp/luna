use super::*;

/// Tests and comparisons: each pairs with the `Jmp` after it.
pub(super) fn validate_branch_op(
    record: &TraceRecord,
    vconsts: &[Option<VConst>],
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    i: usize,
    rop: &RecordedOp,
    op: Op,
    a: usize,
    b: usize,
    consumed_by_cmp: &mut [bool],
    cmp_dirs: &mut [Option<CmpDir>],
) -> Option<()> {
    let oob = |i: usize, r: u32| {
        r as usize >= max_stack && !(r as usize == max_stack && vconst_at(vconsts, i).is_some())
    };
    match op {
        Op::Test => {
            // `if (not R[A] == k) then pc++`. Same
            // direction inference as cmp ops: next.pc==pc+1 + Jmp
            // → TookJmp (test failed); next.pc==pc+2 → SkippedJmp
            // (test passed, K matched).
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if i + 1 >= effective_end {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let next = &record.ops[i + 1];
            let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
            let skipped_jmp = next.pc == rop.pc + 2;
            if took_jmp {
                consumed_by_cmp[i + 1] = true;
                cmp_dirs[i] = Some(CmpDir::TookJmp);
            } else if skipped_jmp {
                let slot = (rop.pc + 1) as usize;
                let jmp_inst = head_proto.code.get(slot).copied();
                if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                cmp_dirs[i] = Some(CmpDir::SkippedJmp);
            } else {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::TestSet => {
            // `if R[B].truthy() == K then R[A]=R[B]
            // else pc++`. R[B] is source; R[A] is move target on
            // test-pass. Direction encoding inverted vs Op::Test:
            //   TookJmp (pc+1 = Jmp) = test passed (no pc++)
            //   SkippedJmp (pc+2)    = test failed (pc++)
            if a >= max_stack || b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if i + 1 >= effective_end {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            let next = &record.ops[i + 1];
            let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
            let skipped_jmp = next.pc == rop.pc + 2;
            if took_jmp {
                consumed_by_cmp[i + 1] = true;
                cmp_dirs[i] = Some(CmpDir::TookJmp);
            } else if skipped_jmp {
                let slot = (rop.pc + 1) as usize;
                let jmp_inst = head_proto.code.get(slot).copied();
                if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                    {
                        checkpoint("bail:cmp-dirs-body-other");
                        return None;
                    }
                }
                cmp_dirs[i] = Some(CmpDir::SkippedJmp);
            } else {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::Lt | Op::Le | Op::Eq => {
            if oob(i, a as u32) || oob(i, b as u32) {
                {
                    checkpoint("bail:cmp-ab-oob");
                    return None;
                }
            }
            // Direction inference: peek at the next recorded
            // op's PC relative to the cmp's PC.
            //
            // - next.pc == cmp.pc + 1 (and it's a Jmp): cmp
            //   matched K → fell through to Jmp → executed it.
            //   `TookJmp` direction. consumed_by_cmp marks
            //   the Jmp.
            // - next.pc == cmp.pc + 2: cmp didn't match K →
            //   pc++ skipped the Jmp slot → continued at
            //   pc + 2. `SkippedJmp` direction. The skipped
            //   Jmp isn't in record.ops; we read its target
            //   from `head_proto.code[cmp.pc + 1]` for the
            //   side-exit PC.
            // - Anything else: bail (cross-block path the
            //   lowerer can't model).
            // relax to `record.ops.len()`:
            // if the Cmp is at `effective_end - 1`, the
            // terminator at `record.ops[effective_end]`
            // still gives us a pc we can use for direction
            // inference (took_jmp / skipped_jmp). Only bail
            // when there's literally no next recorded op.
            // The took_jmp path's `consumed_by_cmp[i+1]`
            // write is gated by `i + 1 < effective_end` so
            // we don't mark a terminator op as consumed.
            if i + 1 >= record.ops.len() {
                {
                    checkpoint("bail:cmp-at-record-end");
                    return None;
                }
            }
            let next = &record.ops[i + 1];
            let took_jmp = matches!(next.inst.op(), Op::Jmp) && next.pc == rop.pc + 1;
            let skipped_jmp = next.pc == rop.pc + 2;
            if took_jmp {
                if i + 1 < effective_end {
                    consumed_by_cmp[i + 1] = true;
                }
                cmp_dirs[i] = Some(CmpDir::TookJmp);
            } else if skipped_jmp {
                // Verify the slot we'd resume to (the Jmp)
                // is actually a Jmp in the Proto's bytecode.
                let slot = (rop.pc + 1) as usize;
                let jmp_inst = head_proto.code.get(slot).copied();
                if !jmp_inst.is_some_and(|x| matches!(x.op(), Op::Jmp)) {
                    {
                        checkpoint("bail:cmp-skipped-but-no-jmp-slot");
                        return None;
                    }
                }
                cmp_dirs[i] = Some(CmpDir::SkippedJmp);
            } else {
                {
                    checkpoint("bail:cmp-next-pc-mismatch");
                    return None;
                }
            }
        }
        _ => unreachable!("routed by validate_op"),
    }
    Some(())
}

/// Table and upvalue ops.
pub(super) fn validate_table_op(
    head_proto: Gc<Proto>,
    max_stack: usize,
    op: Op,
    ins: Inst,
    a: usize,
    b: usize,
    c: usize,
) -> Option<()> {
    match op {
        // Table ops — A is the dest / table reg per op; B/C may be
        // immediates (SetI's key, GetI's key, NewTable's hints).
        Op::NewTable => {
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::GetI => {
            // R[A] := R[B][C_imm]
            if a >= max_stack || b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::GetTable => {
            // R[A] := R[B][R[C]]
            if a >= max_stack || b >= max_stack || c >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::SetI => {
            // R[A][B_imm] := R[C]
            if a >= max_stack || c >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::SetTable => {
            // R[A][R[B] or const[B]] := R[C] or const[C].
            // Step-6 only handles the all-reg form (k=false);
            // const RHS goes through different helpers.
            if ins.k() {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if a >= max_stack || b >= max_stack || c >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::SetList => {
            // R[A][C + i] = R[A + i] for i in 1..=B.
            // Step-7 only handles the fixed-count form
            // (B > 0) without the k=true ExtraArg follower
            // (which encodes a >MAX_ABC offset). The element
            // window must fit in the frame.
            if ins.k() || ins.b() == 0 {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if a >= max_stack || a + ins.b() as usize >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::Len => {
            // R[A] := #R[B]
            if a >= max_stack || b >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
        }
        Op::GetUpval => {
            // R[A] := UpVal[B]. The upval index B is bounded by
            // head_proto.upvals.len() at compile time.
            if a >= max_stack {
                {
                    checkpoint("bail:cmp-dirs-body-other");
                    return None;
                }
            }
            if b >= head_proto.upvals.len() {
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
