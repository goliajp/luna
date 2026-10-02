use super::*;

/// Emits the tail, the way the trace closes, and seals the loop head.
/// Returns the down-recursion link and guard count for `CompiledTrace`.
pub(super) fn emit_tail<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
) -> Option<(Option<(u32, u32)>, u8)> {
    let Plan {
        record,
        max_stack,
        call_idx_opt,
        for_loop_idx_opt,
        inline_abort_idx_opt,
        return_idx_opt,
        self_link_idx_opt,
        downrec_idx_opt,
        do_internal_loop,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        body_loop,
        ..
    } = *lw;
    // --- tail.
    //
    // Four cases pick the clean-close shape:
    //
    // - Trace truncated by `Op::Call` → store back + return
    //   `call.pc`. The Call's interp re-execution is the
    //   "exit" — no loop possible.
    // - Trace closes on `Op::ForLoop` (5.4+ Int count form) →
    //   emit the count check + step IR, then either jump back
    //   to `body_loop` (continue path) or side-exit at
    //   `forloop.pc + 1` (loop exit). In one-shot mode the
    //   continue path returns `head_pc` instead, so the
    //   dispatcher gets one iter per entry.
    // - `opts.internal_loop && has_cmp` (and no Call truncation,
    //   no ForLoop) → jump back to `body_loop`. The trace runs
    //   natively until some cmp side-exits; the dispatcher's
    //   per-entry marshal cost amortizes across however many
    //   iterations the loop runs.
    // - Otherwise (one-shot mode or a no-cmp trace) → store back
    //   + return `head_pc`. The dispatcher re-enters per
    //   iteration; an internal loop with no side-exit would
    //   spin forever.
    // every tail `emit_store_back_and_return_pc`
    // passes `&regs_full[..max_stack]` so the store-back ONLY writes
    // the caller's window back to interp stack. Slots at
    // [max_stack..window_size) are inline-frame scratch and must not
    // leak into the dispatcher's reg_state restore.
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    // populated by the `downrec_idx_opt` arm when
    // it emits the stitch sentinel. Flows into `CompiledTrace.
    // downrec_link` at the struct literal below. `None` for every
    // other close shape.
    let mut downrec_link_for_compiled: Option<(u32, u32)> = None;
    let mut downrec_multi_way_count_for_compiled: u8 = 0;
    if let Some((_dr_idx, dr_return_pc, _target_proto_id, _depth_delta)) = downrec_idx_opt {
        (
            downrec_link_for_compiled,
            downrec_multi_way_count_for_compiled,
        ) = emit_downrec_tail(lw, pl, dr_return_pc, _target_proto_id, _depth_delta);
    } else if let Some((_self_link_idx, _kind)) = self_link_idx_opt {
        emit_self_link_tail(lw, pl);
    } else if let Some(call_idx) = call_idx_opt {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[call_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(inline_abort_idx) = inline_abort_idx_opt {
        // InlineAbort: emit-up-to-i, then store back
        // + return record.ops[i].pc. Dispatchable is forced false
        // below (the interp can't resume at a depth>0 PC without the
        // CallFrames the trace inlined past).
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[inline_abort_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(return_idx) = return_idx_opt {
        // Return0/Return1 at depth=0: caller frame
        // unwinds. Same shape as Call truncation — store back caller
        // window + return the Return op's PC so the interp re-executes
        // it with the correct register state. Subject to the same
        // length-gate dispatchable check below.
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.ops[return_idx].pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    } else if let Some(for_loop_idx) = for_loop_idx_opt {
        emit_loop_tail(lw, pl, for_loop_idx)?;
    } else if do_internal_loop && loop_kinds_match(&lw.current_kinds[..max_stack], &lw.head_kinds) {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        lw.bcx.ins().jump(body_loop, &[]);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            record.head_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
    // Seal the loop head now that both predecessors are emitted
    // (entry → body_loop in the prelude; tail → body_loop from
    // whichever block we ended up in for the clean-close case
    // when internal loop is on).
    lw.bcx.seal_block(body_loop);
    Some((
        downrec_link_for_compiled,
        downrec_multi_way_count_for_compiled,
    ))
}

/// `TraceEnd::DownRec`: the caller-pc guard and the stitch sentinel.
pub(super) fn emit_downrec_tail<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    dr_return_pc: u32,
    _target_proto_id: usize,
    _depth_delta: u8,
) -> (Option<(u32, u32)>, u8) {
    let Plan {
        record,
        max_stack,
        window_size_us,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    let mut downrec_link_for_compiled: Option<(u32, u32)> = None;
    let downrec_multi_way_count_for_compiled: u8;
    // `TraceEnd::DownRec` close: emit the
    // stitch-sentinel + caller-pc-guard.
    //
    // Shape mirrors LuaJIT's `asm_retf` (`lj_asm_arm64.h:565`):
    //   1. Load the saved caller PC.
    //   2. CMP against IR-baked candidate caller PCs.
    //   3. brif eq → stitch_blk: return DOWNREC sentinel +
    //      `record.head_pc` so the dispatcher can walk
    //      `downrec_link` + RetfRecord chain to materialise the
    //      inlined frame and tail-call into the stitched child
    //      trace.
    //   4. brif ne → deopt_blk: safe deopt-tail — store
    //      back caller window + return `head_pc` through the
    //      GLOBAL sentinel; the dispatcher resumes interp at
    //      head_pc.
    debug_assert!(
        dr_return_pc != 0,
        "DownRec recorder should never trip on a PC=0 Op::Return — Op::Return's PC is past the prologue"
    );
    let _ = _target_proto_id;
    let _ = _depth_delta;

    let stitch_blk = lw.bcx.create_block();
    let deopt_blk = lw.bcx.create_block();

    // multi-way caller-pc guard. A single CMP
    // (`saved_pc == dr_return_pc`) misses ~90% of the time on a
    // fib(3) hot loop because
    // the typical fib body has TWO call sites at distinct
    // `pc + 1` caller_pcs — only one of them ever matches
    // `dr_return_pc` (the recorder picks the most-recent
    // threshold-tripping one). The recorder's `rec.retfs`
    // side-channel already collected every depth>0 Return's
    // `caller_pc` + `proto`, so the lowerer here can fan the
    // single CMP into a chain of `icmp(Equal, saved_pc, iconst
    // (candidate_pc)) + brif(eq, stitch, next)` predicates and
    // accept any of them as a HIT. Dedupe over `caller_pc`
    // (mirrors LuaJIT `lj_record.c:897 check_downrec_unroll`'s
    // "count IR_RETF entries by op1 == ptref" walk filtered to
    // the close marker's `target_proto`).
    //
    // Saved-PC slot (`reg_state[window_size_us * 8]`) populated
    // by the dispatcher pre-invoke (see `crates/luna-core/src/
    // vm/exec.rs` `is_downrec_entry` block) with the parent
    // (caller) frame's `pc` — the runtime analogue of LuaJIT's
    // `[base-8]` in `asm_retf` (`lj_asm_arm64.h:565`).
    let saved_pc_offset = (window_size_us as i32) * 8;
    let saved_pc = lw.bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        reg_state,
        saved_pc_offset,
    );
    // Collect distinct caller_pcs from retfs whose proto matches
    // the close marker's `_target_proto_id`. Dedupe + bound to
    // `DOWNREC_MULTI_WAY_GUARD_MAX` so IR size stays predictable
    // regardless of how many retfs the recorder captured.
    // `dr_return_pc` (the close marker's most-recent caller_pc) is
    // inserted first so the chain covers the single-CMP shape's
    // baseline even when filtering eliminates it.
    let mut candidates: Vec<u32> = Vec::with_capacity(DOWNREC_MULTI_WAY_GUARD_MAX);
    candidates.push(dr_return_pc);
    for retf in &record.retfs {
        if candidates.len() >= DOWNREC_MULTI_WAY_GUARD_MAX {
            break;
        }
        if retf.proto.as_ptr() as usize == _target_proto_id && !candidates.contains(&retf.caller_pc)
        {
            candidates.push(retf.caller_pc);
        }
    }
    // Emit CMP-chain. For each candidate: `icmp(Equal, ...) + brif`.
    // The last candidate's miss arm branches directly to deopt_blk;
    // earlier candidates' miss arms branch into a fresh block that
    // becomes the next CMP's "current block".
    for (i, candidate_pc) in candidates.iter().enumerate() {
        let imm_pc = lw.bcx.ins().iconst(types::I64, *candidate_pc as i64);
        let eq = lw.bcx.ins().icmp(IntCC::Equal, saved_pc, imm_pc);
        let miss_blk = if i + 1 < candidates.len() {
            lw.bcx.create_block()
        } else {
            deopt_blk
        };
        lw.bcx.ins().brif(eq, stitch_blk, &[], miss_blk, &[]);
        if i + 1 < candidates.len() {
            lw.bcx.switch_to_block(miss_blk);
            lw.bcx.seal_block(miss_blk);
        }
    }
    let multi_way_candidate_count = candidates.len();

    // Hit: return DOWNREC sentinel + `record.head_pc` as the
    // low 32 bits. The full encoded value is
    //   raw_ret = (1u64 << 63)             // side-trace marker
    //           | ((DOWNREC_CODE as u64) << 56)
    //           | (record.head_pc as u64)
    // (bit 63 set so the dispatcher's `from_side_trace` branch
    // at `exec.rs:6354+` decodes through the sentinel switch).
    // The dispatcher's stitch arm reads `parent_ct.downrec_link` for the
    // stitch target rather than looking up via `side_trace_cache`.
    lw.bcx.switch_to_block(stitch_blk);
    lw.bcx.seal_block(stitch_blk);
    let raw_ret = (1u64 << 63) | ((SIDE_SENT_DOWNREC_CODE as u64) << 56) | (record.head_pc as u64);
    let stitch_ret = lw.bcx.ins().iconst(types::I64, raw_ret as i64);
    lw.bcx.ins().return_(&[stitch_ret]);

    // Miss: safe deopt-tail. The interpreter runs the head op before
    // anything enters the trace again: entered at once with the same
    // registers it would miss the same way forever.
    lw.bcx.switch_to_block(deopt_blk);
    lw.bcx.seal_block(deopt_blk);
    let r = lw
        .module
        .declare_func_in_func(suppress_admit_id, lw.bcx.func);
    lw.bcx.ins().call(r, &[]);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        record.head_pc,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );

    // Populate downrec_link with the placeholder
    // (trace_id=0, target_head_pc=record.head_pc). The
    // `trace_id=0` sentinel means "self-stitch — target is the
    // trace currently dispatching"; the dispatcher interprets
    // this when resolving the stitch target.
    //
    // The dispatcher admits a trace with a link even when it is not
    // dispatchable, so a body already marked (a value of unknown
    // type) gets no link: that mark only ever turns the trace off.
    if lw.dispatchable {
        downrec_link_for_compiled = Some((0, record.head_pc));
    }

    // With at least 2 distinct caller_pc candidates the multi-way
    // guard hits often enough for the primary dispatcher arm to
    // admit the trace, so it stays dispatchable (unless its body
    // was already marked). The single-CMP fallback (count == 1)
    // sets `dispatchable = false` + `"downrec-stitch-pending"`
    // because its ~90% miss-rate would translate to 90% extra
    // deopt cost if the primary dispatcher arm admitted the trace
    // unconditionally; the dispatcher's `is_downrec_entry` arm
    // keys on `ct.downrec_link.is_some()`, so a linked trace is
    // still admitted there. The multi-way count is surfaced via
    // the `multi_way_guard_emitted` counter, bumped at the close
    // handler from `downrec_multi_way_count_for_compiled` below.
    if multi_way_candidate_count < 2 {
        lw.dispatchable = false;
        lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("downrec-stitch-pending"));
    }
    downrec_multi_way_count_for_compiled = multi_way_candidate_count.min(u8::MAX as usize) as u8;
    (
        downrec_link_for_compiled,
        downrec_multi_way_count_for_compiled,
    )
}

/// A self-link close: a deopt tail.
pub(super) fn emit_self_link_tail<M: Module>(lw: &mut Lower<'_, '_, M>, pl: &Plan<'_>) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    // Self-link close deopts instead of looping natively.
    //
    // A native tail (slot-copy `regs_full[i] = regs_full[bump_off
    // + i]` for `i in 0..max_stack`, deepest inlined frame → head
    // frame, then `jump(body_loop)`) would mirror LuaJIT's
    // `asm_tail_link` (`lj_asm.c:2131`) only
    // syntactically. LuaJIT's pre-op snapshots distinguish each
    // frame's typed-slot mapping; luna's slot-copy assumes deepest
    // frame layout == head frame layout, which is sound for plain
    // tail-recursion but not for self-recursion through a
    // non-tail-call body (fib: `Lt → branch → Sub Call Sub Call Add
    // Return`, with depth-0 Sub writes polluting head-frame slots
    // BEFORE the recursive Call, plus a depth>0 base-case Return
    // whose deeper frame layout doesn't match head's). With that
    // tail fib(28) returns 45 instead of 317_811.
    //
    // So the tail is a clean deopt: store
    // back the caller window, return `head_pc`, and pin
    // `dispatchable = false`. The trace still compiles (cranelift
    // accepts a valid back-edge-free fn so the body's mcode and
    // window_size extension stay sound) but the dispatcher's
    // pre-invoke `dispatchable` check refuses to enter it, so
    // interp runs the recursion naturally and produces the correct
    // result.
    //
    // The `RetfRecord` side-channel populated by the recorder
    // (exec.rs gate on `self_link_enabled`) captures the
    // inlined-frame topology that the down-rec stitch consumes
    // to guard a real native back-edge.
    //
    // The `window_size_us` extension above (`record.self_link
    // _kind.is_some()` arm) stays intact — body emit still writes
    // depth>0 slots into the extended buffer; the writes are
    // simply dead here. As for the downrec miss, the interpreter runs
    // the head op before the trace is entered again.
    let r = lw
        .module
        .declare_func_in_func(suppress_admit_id, lw.bcx.func);
    lw.bcx.ins().call(r, &[]);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        record.head_pc,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );
    lw.dispatchable = false;
    lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("self-link-retf-r1"));
}

/// The loop edge at `effective_end`.
pub(super) fn emit_loop_tail<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    for_loop_idx: usize,
) -> Option<()> {
    let Plan { record, .. } = *pl;
    // 5.4+ Int count form (validated above; pre53 bails).
    //
    //   if R[A+1] > 0:
    //     R[A]     = R[A] + R[A+2]    (next loop var)
    //     R[A+1]   = R[A+1] - 1       (decrement count)
    //     R[A+3]   = R[A]              (visible loop var copy)
    //     // continue → back-edge (body_loop) or return head_pc
    //   else:
    //     // exit → side-exit at forloop.pc + 1
    //
    // ForLoop is only set at depth=0 (ForLoop@d>0 closes via
    // InlineAbort), so `regs_full[a]` directly addresses the
    // caller window — no offset.
    let rop = &record.ops[for_loop_idx];
    let a = rop.inst.a() as usize;
    match rop.inst.op() {
        Op::ForLoop => emit_for_loop_tail(lw, pl, rop, a),
        Op::TForLoop => emit_tfor_loop_tail(lw, pl, for_loop_idx, rop, a)?,
        _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
    }
    Some(())
}

/// `Op::ForLoop` (the 5.4+ integer count form).
pub(super) fn emit_for_loop_tail<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    rop: &RecordedOp,
    a: usize,
) {
    let Plan {
        record,
        max_stack,
        do_internal_loop,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        body_loop,
        ..
    } = *lw;
    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    let count = lw.bcx.use_var(lw.regs_full[a + 1]);
    let zero = lw.bcx.ins().iconst(types::I64, 0);
    // the loop count is unsigned (PUC `lua_Unsigned`)
    let cond = lw.bcx.ins().icmp(IntCC::NotEqual, count, zero);

    let continue_blk = lw.bcx.create_block();
    let exit_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(cond, continue_blk, &[], exit_blk, &[]);

    // exit branch: side-exit at forloop.pc + 1.
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        rop.pc + 1,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );

    // continue branch: do the increment + decrement + back-edge.
    lw.bcx.switch_to_block(continue_blk);
    lw.bcx.seal_block(continue_blk);
    let cur = lw.bcx.use_var(lw.regs_full[a]);
    let step = lw.bcx.use_var(lw.regs_full[a + 2]);
    let next = lw.bcx.ins().iadd(cur, step);
    lw.bcx.def_var(lw.regs_full[a], next);
    let one = lw.bcx.ins().iconst(types::I64, 1);
    let count_new = lw.bcx.ins().isub(count, one);
    lw.bcx.def_var(lw.regs_full[a + 1], count_new);
    lw.bcx.def_var(lw.regs_full[a + 3], next);
    // ForLoop's continue branch jumps to the loop's
    // BODY START (= (rop.pc + 1) - bx per OP_FORLOOP's
    // backward jump encoding), not record.head_pc.
    // For trace shapes whose head_pc == body_start
    // (the usual back-edge trace), they're equal.
    // For side traces whose head_pc lands on the
    // ForLoop op itself (head_pc=rop.pc) instead of
    // the back-edge target — e.g. an outer ForLoop
    // that got recorded as a side trace from an inner
    // loop exit — returning record.head_pc would
    // re-enter the ForLoop op and double-advance the
    // counter. A trace headed at an inner loop (a
    // `while` inside the for body) that closes at the
    // outer ForLoop must not loop back to its own head
    // either: that skips the body code before the inner
    // loop. Compute the body start explicitly.
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    let mut tail_kinds = lw.current_kinds[..max_stack].to_vec();
    for k in [a, a + 1, a + 3] {
        tail_kinds[k] = RegKind::Int;
    }
    if do_internal_loop
        && body_pc == record.head_pc
        && loop_kinds_match(&tail_kinds, &lw.head_kinds)
    {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        lw.bcx.ins().jump(body_loop, &[]);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            body_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
}

/// `Op::TForLoop`, the generic-for back-edge.
pub(super) fn emit_tfor_loop_tail<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    for_loop_idx: usize,
    rop: &RecordedOp,
    a: usize,
) -> Option<()> {
    let Plan {
        record,
        max_stack,
        do_internal_loop,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        tforcall_tag_var,
        tforcall_val_tag_var,
        body_loop,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    // the caller window: see `emit_tail`
    let caller_regs: &[Variable] = &lw.regs_full[..max_stack];
    // generic-for back-edge:
    //
    //   tag = tforcall_tag_var  // from TForCall's
    //                           //     batched helper
    //                           //     return value
    //   if tag == NIL:  side-exit at tforloop.pc + 1
    //   elif the key's (and value's) tag is the one the
    //        body was compiled for: R[A+2]=R[A+4] +
    //        back-edge
    //   else: deopt (the interpreter runs the TForLoop)
    //
    // The Nil branch reuses the existing dispatcher
    // restore path; push a per_exit_kinds snapshot with
    // [A+4] = RegKind::Nil so the Nil side-exit repacks
    // correctly (entry's tag for A+4 was Int, so
    // dispatcher without override would restore as Int
    // — wrong for Nil).
    let tag = lw.bcx.use_var(tforcall_tag_var);
    // The body was lowered for the head's entry tags; the
    // back-edge runs it again only with a key (and, when the
    // loop has one, a value) of those tags. A pairs loop
    // over string keys meeting an integer key (or the
    // reverse) stored the new key under the old tag.
    let nvars = match record.ops[for_loop_idx - 1].inst.op() {
        Op::TForCall => record.ops[for_loop_idx - 1].inst.c() as usize,
        _ => return None,
    };
    let key_tag = *record.entry_tags.get(a + 4)?;
    let val_tag = if nvars >= 2 {
        Some(*record.entry_tags.get(a + 5)?)
    } else {
        None
    };

    let nil_const = lw
        .bcx
        .ins()
        .iconst(types::I64, luna_core::runtime::value::raw::NIL as i64);
    let is_nil = lw.bcx.ins().icmp(IntCC::Equal, tag, nil_const);
    let nil_exit_blk = lw.bcx.create_block();
    let not_nil_blk = lw.bcx.create_block();
    lw.bcx
        .ins()
        .brif(is_nil, nil_exit_blk, &[], not_nil_blk, &[]);

    // Nil-exit branch: snapshot per_exit_kinds with [A+4]
    // = Nil, then store back + return tforloop.pc + 1.
    lw.bcx.switch_to_block(nil_exit_blk);
    lw.bcx.seal_block(nil_exit_blk);
    // Every loop variable restores as nil: the key is nil, and
    // the value slots hold what the iterator's last call left
    // (nil in the helper path), which the loop no longer reads.
    let mut nil_snapshot: Vec<RegKind> = lw.current_kinds[..max_stack].to_vec();
    for k in (a + 4)..(a + 4 + nvars).min(nil_snapshot.len()) {
        nil_snapshot[k] = RegKind::Nil;
    }
    let tag_side_box_2: Box<TCellPtr> = Box::new(TCellPtr::null());
    let _tag_side_cell_addr_2 = (&*tag_side_box_2) as *const TCellPtr as i64;
    let tag_side_local_2 = lw.per_exit_kinds.len() as u32;
    lw.per_exit_kinds
        .push((rop.pc + 1, nil_snapshot, tag_side_box_2));
    emit_tagged_exit(
        &mut lw.bcx,
        &mut lw.module,
        suppress_admit_id,
        caller_regs,
        &lw.stored,
        reg_state,
        rop.pc + 1,
        record.head_pc,
        tag_side_local_2,
        lw.flush_ctx.as_ref(),
        trace_fn_sig_ref,
    );

    lw.bcx.switch_to_block(not_nil_blk);
    lw.bcx.seal_block(not_nil_blk);
    let mut same_kinds = lw
        .bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, tag, i64::from(key_tag));
    if let Some(val_tag) = val_tag {
        let v = lw.bcx.use_var(tforcall_val_tag_var);
        let same_val = lw.bcx.ins().icmp_imm_u(IntCC::Equal, v, i64::from(val_tag));
        same_kinds = lw.bcx.ins().band(same_kinds, same_val);
    }
    let continue_blk = lw.bcx.create_block();
    let deopt_blk = lw.bcx.create_block();
    lw.bcx
        .ins()
        .brif(same_kinds, continue_blk, &[], deopt_blk, &[]);

    // Deopt: the next key or value has another kind than the
    // body was compiled for. Store back + return TForLoop.pc
    // so the interp re-executes the back-edge.
    lw.bcx.switch_to_block(deopt_blk);
    lw.bcx.seal_block(deopt_blk);
    // The helper already wrote the loop variables to the stack
    // with their tags, which are not the ones the registers
    // were compiled for; the dispatcher must leave them there.
    emit_store_back_and_return(
        &mut lw.bcx,
        caller_regs,
        &lw.stored,
        reg_state,
        (luna_core::jit::trace_types::EXIT_KEEP_TFOR_VARS | u64::from(rop.pc)) as i64,
        lw.flush_ctx.as_ref(),
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
    );

    // Continue: R[A+2] = R[A+4] (ctrl writeback) +
    // back-edge / store_back+head_pc.
    lw.bcx.switch_to_block(continue_blk);
    lw.bcx.seal_block(continue_blk);
    let ctrl = lw.bcx.use_var(lw.regs_full[a + 4]);
    lw.bcx.def_var(lw.regs_full[a + 2], ctrl);
    // as for ForLoop: continue at the loop body, which is
    // the trace head only when the trace was recorded from it
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    // the loop variables passed the tag check above, and the
    // control variable is a copy of the key
    let mut tail_kinds = lw.current_kinds[..max_stack].to_vec();
    let vars = (a + 4)..(a + 4 + nvars.min(2)).min(max_stack);
    tail_kinds[vars.clone()].copy_from_slice(&lw.head_kinds[vars]);
    tail_kinds[a + 2] = lw.head_kinds[a + 4];
    if do_internal_loop
        && body_pc == record.head_pc
        && loop_kinds_match(&tail_kinds, &lw.head_kinds)
    {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        lw.bcx.ins().jump(body_loop, &[]);
    } else {
        emit_store_back_and_return_pc(
            &mut lw.bcx,
            caller_regs,
            &lw.stored,
            reg_state,
            body_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
            encode_side_sentinel(SIDE_SENT_KIND_GLOBAL, 0),
        );
    }
    Some(())
}
