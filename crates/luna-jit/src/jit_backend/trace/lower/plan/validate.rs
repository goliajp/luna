use super::*;

/// The per-op pre-emit contract over the body: every op must be one
/// the emit pass lowers, with its operands in range. Records which `Jmp`s
/// a preceding test consumes and which way each test went.
pub(super) fn validate_ops(
    record: &TraceRecord,
    vconsts: &[VRegs],
    head_proto: Gc<Proto>,
    max_stack: usize,
    effective_end: usize,
    folded_ops: &[bool],
    frame_tops: &[Option<u32>],
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
        // a list store of the values a call just returned: the recording
        // fixes their count (`inline_calls`), which the store takes from
        // the recorded `var_count`
        if rop.inst.op() == Op::SetList && rop.inst.b() == 0 && !folded_ops[i] {
            set_last_op(i, rop.inst.op() as u8);
            let a = rop.inst.a();
            let fixed = frame_tops
                .get(i)
                .copied()
                .flatten()
                .and_then(|t| t.checked_sub(a + 1))
                .filter(|&n| Some(n) == rop.var_count && (a + n) as usize <= max_stack);
            if fixed.is_none() || rop.inst.k() {
                checkpoint("bail:setlist-count-not-fixed");
                return None;
            }
            continue;
        }
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
    vconsts: &[VRegs],
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
    set_last_op(i, rop.inst.op() as u8);
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
    if rop.inline_depth > 0 && matches!(op, Op::Return0 | Op::Return1 | Op::Return) {
        // the values returned sit in the frame (`inline_calls` fixed the
        // count of a `Return`)
        let n = match op {
            Op::Return0 => 0,
            Op::Return1 => 1,
            _ if rop.inst.b() > 0 => rop.inst.b() - 1,
            _ => 1,
        };
        if (rop.inst.a() + n) as usize > max_stack {
            checkpoint("bail:cmp-dirs-Return1-a-oob");
            return None;
        }
        return Some(());
    }
    // a vararg expansion in a vararg function the trace inlined: the extra
    // arguments sit below the frame's registers, as many as the call
    // passed (the head frame's are on the stack, which the trace does not
    // read)
    if matches!(op, Op::Vararg) {
        let fits = rop.inline_depth > 0 && rop.proto.is_vararg && {
            let n = rop.inst.c().saturating_sub(1);
            (rop.inst.a() + n) as usize <= max_stack
        };
        if !fits {
            checkpoint("bail:vararg-outside-inlined-frame");
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
        Op::ForLoop | Op::ForLoop55 => {
            unreachable!("Op::ForLoop only appears at effective_end (loop-end guarded above)")
        }
        Op::TForLoop | Op::TForLoop53 | Op::TForLoop55 => unreachable!(
            "Op::TForLoop only appears at effective_end (close-on-back-edge guarded above)"
        ),
        Op::TForPrep
        | Op::TForPrep53
        | Op::TForPrep55
        | Op::TForCall
        | Op::TForCall53
        | Op::TForCall55
        | Op::Concat
        | Op::GetTabUp
        | Op::SetField
        | Op::GetField
        | Op::Jmp
        | Op::JmpClose
        | Op::JmpCloseBack
        | Op::Move
        | Op::LoadI
        | Op::LoadF
        | Op::LoadNil
        | Op::LoadFalse
        | Op::LoadTrue
        | Op::LFalseSkip
        | Op::LTrueSkip
        | Op::Not
        | Op::Close => validate_body_op(vconsts, i, max_stack, rop, op, ins, a, b, c)?,
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
        | Op::GetUpval => validate_table_op(head_proto, vconsts, i, max_stack, op, ins, a, b, c)?,
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
