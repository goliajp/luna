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
    // an inlined function's op reads its own constants, nested
    // functions and upvalue descriptions
    let _ = head_proto;
    let head_proto = rop.proto;
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
        | Op::LoadFalse
        | Op::LoadTrue
        | Op::LFalseSkip
        | Op::Not
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
        Op::SelfOp => {
            // a constant string key (the register form is the compiler's
            // fallback for a constant past C's range)
            let key_is_str = ins.k()
                && matches!(
                    head_proto.consts.get(c),
                    Some(luna_core::runtime::Value::Str(_))
                );
            if !key_is_str || a + 1 >= max_stack || b >= max_stack {
                checkpoint("bail:self-op-shape");
                return None;
            }
        }
        _ => unreachable!("whitelist gated above"),
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
        if matches!(rop.inst.op(), Op::Jmp)
            && !consumed_by_cmp[i]
            && i + 1 != effective_end
            && !jumps_to_next(rop, &record.ops[i + 1])
        {
            checkpoint("bail:body-jmp");
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

/// A forward `Jmp` (the end of an `if` branch skipping the `else`) that
/// the recording followed to `next`: the trace goes on there, and the jump
/// needs no code.
fn jumps_to_next(jmp: &RecordedOp, next: &RecordedOp) -> bool {
    let target = i64::from(jmp.pc) + 1 + i64::from(jmp.inst.sj());
    jmp.inst.sj() >= 0
        && next.inline_depth == jmp.inline_depth
        && std::ptr::eq(next.proto.as_ptr(), jmp.proto.as_ptr())
        && i64::from(next.pc) == target
}
