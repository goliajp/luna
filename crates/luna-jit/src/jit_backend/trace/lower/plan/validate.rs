use super::*;

/// The per-op pre-emit contract over the body: every op must be one
/// the emit pass lowers, with its operands in range. Records which `Jmp`s
/// a preceding test consumes and which way each test went.
pub(super) fn validate_ops(
    record: &TraceRecord,
    vconsts: &[Option<VConst>],
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    folded_ops: &[bool],
) -> Option<(Vec<bool>, Vec<Option<CmpDir>>)> {
    // Pre-emit verification. Any op outside the whitelist contract
    // bails so the trace becomes a no-op (the recorder counts it
    // toward the head PC's failure count and won't re-record
    // unless the back-edge counter rolls over again).
    //
    // `consumed_by_cmp[i] = true` marks ops[i] as a `Jmp` whose
    // sole role is to be the recorded post-cmp branch — the cmp's
    // `brif` already carries its control transfer, so emit skips
    // the Jmp's IR entirely.
    let mut consumed_by_cmp = vec![false; effective_end];
    // Parallel to record.ops: for each cmp op, which direction
    // was recorded? `None` for non-cmps; set by the pre-emit pass
    // and consumed by body emit.
    let mut cmp_dirs: Vec<Option<CmpDir>> = vec![None; effective_end];
    checkpoint("pre:cmp-dirs-loop");
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        validate_op(
            record,
            vconsts,
            head_proto,
            max_stack,
            effective_end,
            folded_ops,
            i,
            rop,
            &mut consumed_by_cmp,
            &mut cmp_dirs,
        )?;
    }
    Some((consumed_by_cmp, cmp_dirs))
}

/// One op of [`validate_ops`].
fn validate_op(
    record: &TraceRecord,
    vconsts: &[Option<VConst>],
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    folded_ops: &[bool],
    i: usize,
    rop: &RecordedOp,
    consumed_by_cmp: &mut [bool],
    cmp_dirs: &mut [Option<CmpDir>],
) -> Option<()> {
    // Folded math-fold ops are validated by the matcher above;
    // the per-op contract here would reject GetTabUp /
    // GetField / Move (dst > A semantic) so skip them.
    if folded_ops[i] {
        return Some(());
    }
    // depth>0 ops are allowed inside the inline
    // self-recursion path. `end_idx_opt` already guards the path
    // (cmp@d>0 / ForLoop@d>0 / non-self Call / proto mismatch /
    // depth past MAX_INLINE_DEPTH all close the trace before they
    // hit emit), so any op reaching this point with depth>0 is
    // a same-proto inline body op the lowerer can handle.
    // capture op_id BEFORE per-op checks for
    // failure-phase narrowing.
    set_last_op_id(rop.inst.op() as u8);
    if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
        checkpoint("bail:cmp-dirs-cross-proto-op");
        return None;
    }
    let op = rop.inst.op();
    // self-recursive Op::Call inside the inline
    // path emits no IR (the next op shifts to the callee window
    // via op_offsets). It's not in `is_whitelisted_op`, so
    // accept it explicitly when depth>0 OR when the next op is
    // at depth+1 (the recorder's self-recursive marker).
    if matches!(op, Op::Call) {
        // The terminator pass already let this Op::Call past as
        // a self-recursive call. Skip the whitelist check.
        return Some(());
    }
    // Op::Return0 / Op::Return1 at depth>0 are
    // the inline path's unwind ops. They're not in the
    // whitelist (it only covers depth=0 ops with no return
    // semantics); admit them when depth>0.
    if rop.inline_depth > 0 && matches!(op, Op::Return0 | Op::Return1) {
        // Bound the A operand for Return1 — Return0 has no A read.
        if matches!(op, Op::Return1) && (rop.inst.a() as usize) >= max_stack {
            checkpoint("bail:cmp-dirs-Return1-a-oob");
            return None;
        }
        return Some(());
    }
    if !is_whitelisted_op(op) {
        checkpoint("bail:cmp-dirs-op-not-whitelisted");
        return None;
    }
    // Op::GetField is lowered standalone via
    // luna_jit_table_get_field (string key from Proto.consts).
    // Op::GetTabUp also lowered standalone via
    // luna_jit_op_get_tab_up. Both require K[C] = Str at compile
    // time (the const-pool string key is baked into IR). GetTabUp
    // additionally pins B = upvalue index in the trace head
    // closure; the helper resolves it via JIT_CL TLS at runtime.
    if matches!(op, Op::GetTabUp) {
        let cx = rop.inst.c() as usize;
        if cx >= head_proto.consts.len()
            || !matches!(head_proto.consts[cx], luna_core::runtime::Value::Str(_))
        {
            checkpoint("bail:cmp-dirs-GetTabUp-key-not-str");
            return None;
        }
    }
    // Op::SetField uses K[B] as string key,
    // Op::GetField uses K[C]. Pre-emit verifies the const is Str.
    if matches!(op, Op::SetField) {
        let bx = rop.inst.b() as usize;
        if bx >= head_proto.consts.len()
            || !matches!(head_proto.consts[bx], luna_core::runtime::Value::Str(_))
        {
            return None;
        }
    }
    if matches!(op, Op::GetField) {
        let cx = rop.inst.c() as usize;
        if cx >= head_proto.consts.len()
            || !matches!(head_proto.consts[cx], luna_core::runtime::Value::Str(_))
        {
            {
                checkpoint("bail:cmp-dirs-body-other");
                return None;
            }
        }
    }
    let ins = rop.inst;
    let a = ins.a() as usize;
    let b = ins.b() as usize;
    let c = ins.c() as usize;
    match op {
        Op::Call => {
            unreachable!("Op::Call only appears at effective_end (truncation guarded above)")
        }
        Op::ForLoop => {
            unreachable!("Op::ForLoop only appears at effective_end (loop-end guarded above)")
        }
        Op::TForLoop => unreachable!(
            "Op::TForLoop only appears at effective_end (close-on-back-edge guarded above)"
        ),
        Op::TForPrep
        | Op::TForCall
        | Op::Concat
        | Op::GetTabUp
        | Op::SetField
        | Op::GetField
        | Op::Jmp
        | Op::Move
        | Op::LoadI
        | Op::LoadF
        | Op::LoadNil
        | Op::Close => validate_body_op(max_stack, rop, op, ins, a, b, c)?,
        Op::Closure
        | Op::LoadK
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Pow
        | Op::IDiv
        | Op::Mod
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr
        | Op::Unm
        | Op::BNot
        | Op::EqK => validate_value_op(
            record,
            vconsts,
            head_proto,
            max_stack,
            effective_end,
            i,
            rop,
            op,
            ins,
            a,
            b,
            c,
            consumed_by_cmp,
        )?,
        Op::Test | Op::TestSet | Op::Lt | Op::Le | Op::Eq => validate_branch_op(
            record,
            vconsts,
            head_proto,
            max_stack,
            effective_end,
            i,
            rop,
            op,
            a,
            b,
            consumed_by_cmp,
            cmp_dirs,
        )?,
        Op::NewTable
        | Op::GetI
        | Op::GetTable
        | Op::SetI
        | Op::SetTable
        | Op::SetList
        | Op::Len
        | Op::GetUpval => validate_table_op(head_proto, max_stack, op, ins, a, b, c)?,
        _ => unreachable!("whitelist gated above"),
    }
    Some(())
}

/// Ops lowered through a helper or as plain moves and loads.
fn validate_body_op(
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
fn validate_value_op(
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

/// Tests and comparisons: each pairs with the `Jmp` after it.
fn validate_branch_op(
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
fn validate_table_op(
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

/// The ops that end the body: stray `Jmp`s, and the truncating call,
/// the depth-0 return or the loop edge at `effective_end`.
#[allow(clippy::too_many_arguments)]
pub(super) fn validate_trace_ends(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    max_stack: usize,
    opts: CompileOptions,
    effective_end: usize,
    consumed_by_cmp: &[bool],
    call_idx_opt: Option<usize>,
    return_idx_opt: Option<usize>,
    for_loop_idx_opt: Option<usize>,
) -> Option<()> {
    // Jmp validation inside the normal range. A Jmp is OK if it
    // was consumed by a preceding cmp (handled above) or sits at
    // the effective end's last position (the back-edge that closes
    // the loop, or the slot right before an Op::Call truncation —
    // the tail / side-exit emits the control transfer).
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        if matches!(rop.inst.op(), Op::Jmp) && !consumed_by_cmp[i] && i + 1 != effective_end {
            return None;
        }
    }

    // Validate the truncating Op::Call (if any). Self-recursion is
    // not verified — the recorder is trusted to only feed sound
    // patterns.
    if let Some(call_idx) = call_idx_opt {
        // call_idx_opt only set for non-self
        // Op::Call at depth 0 (self-recursive inline calls pass
        // through end_idx_opt without truncating; depth>0 closures
        // close via TraceEnd::InlineAbort), so the depth check below
        // is only a debug assert.
        let rop = &record.ops[call_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::Call only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        let a = rop.inst.a() as usize;
        if a >= max_stack {
            return None;
        }
    }

    // Validate the trailing Op::ForLoop (if any). Step-6 only
    // lowers the 5.4+ Int count form — pre-5.3 compares R[A+1]
    // directly against `limit` and uses a different state slot
    // layout, so traces from those dialects bail. The recorder
    // is trusted that R[A..A+3] really do hold Ints at runtime;
    // the dispatcher's all-Int marshal gate enforces that
    // separately on the call boundary.
    // validate Op::Return0/Return1 at depth=0
    // (TraceEnd::Return). Same A bound rule as Call truncation
    // applies to Return1; Return0 has no A read.
    if let Some(return_idx) = return_idx_opt {
        let rop = &record.ops[return_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::Return only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        if matches!(rop.inst.op(), Op::Return1) {
            let a = rop.inst.a() as usize;
            if a >= max_stack {
                return None;
            }
        }
    }

    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        debug_assert_eq!(rop.inline_depth, 0, "TraceEnd::ForLoop only at depth 0");
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        let a = rop.inst.a() as usize;
        match rop.inst.op() {
            Op::ForLoop => {
                if opts.pre53 {
                    return None;
                }
                // ForLoop touches R[A], R[A+1] (count), R[A+2] (step),
                // R[A+3] (visible loop var). All must fit in the frame.
                if a + 3 >= max_stack {
                    return None;
                }
                // Bail on Float ForLoop. Trace JIT's emit at line ~7233
                // reads R[A+1] as Int count + tests `count > 0`. For
                // Float ForLoop (5.4+ Float-counter form), R[A+1] is
                // the LIMIT (Float bits), not a remaining-iteration
                // count. The Int-semantics check would treat the float
                // bits as a large positive integer (always > 0) and
                // loop forever inside the trace. PUC's interp handles
                // Float and Int ForLoop with separate semantics; the
                // trace JIT only emits the Int path correctly.
                if a < record.entry_tags.len()
                    && record.entry_tags[a] == luna_core::runtime::value::raw::FLOAT
                {
                    return None;
                }
            }
            Op::TForLoop => {
                // TForLoop reads R[A+4] (control
                // returned by the iterator) and writes R[A+2] on
                // continue. R[A+4] must fit in the trace's frame.
                if a + 4 >= max_stack {
                    return None;
                }
            }
            _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
        }
    }
    Some(())
}
