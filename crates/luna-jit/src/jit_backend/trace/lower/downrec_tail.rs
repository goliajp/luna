use super::*;

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
    let downrec_multi_way_count_for_compiled =
        multi_way_candidate_count.min(u8::MAX as usize) as u8;
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
