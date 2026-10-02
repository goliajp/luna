use super::*;

// Call a checked read helper; on failure leave the trace at `$pc`,
// otherwise evaluate to the payload it wrote.
macro_rules! checked_read {
    ($lw:ident, $pl:ident, $id:expr, $a0:expr, $a1:expr, $want:expr, $pc:expr, $i:expr) => {{
        let out_ss = $lw
            .bcx
            .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
        let out_addr = $lw.bcx.ins().stack_addr(types::I64, out_ss, 0);
        let want = $lw.bcx.ins().iconst(types::I64, $want as i64);
        let fref = $lw.module.declare_func_in_func($id, $lw.bcx.func);
        let call = $lw.bcx.ins().call(fref, &[$a0, $a1, want, out_addr]);
        let ok = $lw.bcx.inst_results(call)[0];
        let cont_blk = $lw.bcx.create_block();
        let exit_blk = $lw.bcx.create_block();
        $lw.bcx.ins().brif(ok, cont_blk, &[], exit_blk, &[]);
        $lw.bcx.switch_to_block(exit_blk);
        $lw.bcx.seal_block(exit_blk);
        guard_exit($lw, $pl, $pc, $i);
        $lw.bcx.switch_to_block(cont_blk);
        $lw.bcx.seal_block(cont_blk);
        $lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 0)
    }};
}
// Continue in a new block when `$cond` holds, else take a
// `guard_exit!` to `$pc`.
macro_rules! guard {
    ($lw:ident, $pl:ident, $cond:expr, $i:expr, $pc:expr) => {{
        let continue_blk = $lw.bcx.create_block();
        let exit_blk = $lw.bcx.create_block();
        $lw.bcx.ins().brif($cond, continue_blk, &[], exit_blk, &[]);
        $lw.bcx.switch_to_block(exit_blk);
        $lw.bcx.seal_block(exit_blk);
        guard_exit($lw, $pl, $pc, $i);
        $lw.bcx.switch_to_block(continue_blk);
        $lw.bcx.seal_block(continue_blk);
    }};
}

mod body;
mod exit;
mod fold;
mod helpers;
mod ops;
mod plan;
mod prologue;
use body::*;
use exit::*;
use fold::*;
use helpers::*;
use ops::*;
use plan::*;
use prologue::*;

/// The trace function under construction and everything the emit pass
/// tracks while lowering it; the fields keep the names the single
/// lowering function used for its locals.
struct Lower<'f, 'm, M: Module> {
    module: &'m mut M,
    bcx: FunctionBuilder<'f>,
    h: Helpers,
    reg_state: Value,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    global_side_trace_box: Box<TCellPtr>,
    regs_full: Vec<Variable>,
    tforcall_tag_var: Variable,
    tforcall_val_tag_var: Variable,
    precheck: Option<Block>,
    body_loop: Block,
    head_kinds: Vec<RegKind>,
    defined_aot_data: std::collections::HashSet<DataId>,
    escape: EscapeAnalysis,
    flush_ctx: Option<FlushCtx>,
    virt_vars: Vec<Option<Vec<Variable>>>,
    virt_kinds: Vec<Option<Vec<RegKind>>>,
    sunk_alloc_seen: u32,
    materialize_emit_count: u32,
    closure_seen: u32,
    stored: Vec<Option<Value>>,
    current_kinds: Vec<RegKind>,
    dispatchable: bool,
    dispatch_off_reason: Option<&'static str>,
    per_exit_kinds: Vec<(u32, Vec<RegKind>, Box<TCellPtr>)>,
    per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )>,
    call_chain: Vec<FrameMaterializeInfo>,
    upval_cache: std::collections::HashMap<u32, Variable>,
    head_closure_var: Option<Variable>,
    known_int: Vec<Option<i64>>,
}

/// `always_codegen = false` leaves the function undefined in `module`
/// when [`trace_is_enterable`] says nothing will run it; `float_only` as
/// in [`compile_trace_jit`].
pub(super) fn lower_trace_into_inner<M: Module>(
    module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
    aot_fn_name: Option<&str>,
    always_codegen: bool,
    float_only: bool,
) -> Option<(FuncId, CompiledTrace)> {
    checkpoint("enter");
    if !record.closed {
        checkpoint("bail:not-closed");
        return None;
    }
    checkpoint("post:closed-check");

    // track which AOT data slots
    // we've already `define_data`'d this lower call. `declare_data`
    // returns the same `DataId` for the same name (Cranelift name
    // interning), but `define_data` rejects redefinition with
    // `ModuleError::DuplicateDefinition` — so the dedupe guard sits
    // around `define_data`, not `declare_data`.
    let defined_aot_data: std::collections::HashSet<DataId> = std::collections::HashSet::new();

    let head_proto = record.head_proto;
    let max_stack = head_proto.max_stack as usize;
    // Every pass below reads register operands: a constant- or
    // immediate-operand op is lowered as its register form with the
    // constant in virtual register `max_stack` (one past the op's frame,
    // never stored back), whose kind and value `vconsts` holds.
    let translated;
    let (record, vconsts) = match split_const_operands(record, max_stack as u32) {
        Some((t, v)) => {
            translated = t;
            (&translated, v)
        }
        None => (record, Vec::new()),
    };
    let (plan, escape) = plan_trace(record, vconsts, head_proto, max_stack, opts, float_only)?;
    let pl = &plan;
    let h = declare_helpers(module)?;

    let mut sig = module.make_signature();
    // Param 0 — reg_state ptr (caller-owned, lives across the call).
    sig.params.push(AbiParam::new(types::I64));
    // Return — continuation PC (head_pc on clean close).
    sig.returns.push(AbiParam::new(types::I64));
    // caller-provided name +
    // export linkage when driving the AOT pipeline. The JIT wrapper
    // (`try_compile_trace_with_options`) passes `None`, preserving the
    // original `luna_jit_trace` / `Linkage::Local` shape.
    let (trace_fn_name, trace_fn_linkage) = match aot_fn_name {
        Some(name) => (name, Linkage::Export),
        None => ("luna_jit_trace", Linkage::Local),
    };
    let fn_id = module
        .declare_function(trace_fn_name, trace_fn_linkage, &sig)
        .ok()?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());

    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

    let head = emit_entry(&mut bcx, module, pl);
    let mut escape = escape;
    let sunk = alloc_sunk_sites(&mut bcx, pl, &mut escape);
    let flush_ctx = start_accum(&mut bcx, module, pl, h, &head.regs_full);
    let blocks = open_body_loop(&mut bcx, pl);
    let mut lower = begin_body(
        module,
        bcx,
        pl,
        h,
        head,
        escape,
        defined_aot_data,
        sunk,
        flush_ctx,
        blocks,
    );
    let lw = &mut lower;
    let Plan {
        record,
        max_stack,
        window_size_us,
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
        tforcall_tag_var,
        tforcall_val_tag_var,
        body_loop,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    emit_fold_precheck(lw, pl);
    emit_body(lw, pl)?;

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
            if retf.proto.as_ptr() as usize == _target_proto_id
                && !candidates.contains(&retf.caller_pc)
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
        let raw_ret =
            (1u64 << 63) | ((SIDE_SENT_DOWNREC_CODE as u64) << 56) | (record.head_pc as u64);
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
        downrec_multi_way_count_for_compiled =
            multi_way_candidate_count.min(u8::MAX as usize) as u8;
    } else if let Some((_self_link_idx, _kind)) = self_link_idx_opt {
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
            Op::ForLoop => {
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
            Op::TForLoop => {
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
            }
            _ => unreachable!("for_loop_idx_opt only set for Op::ForLoop / Op::TForLoop"),
        }
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
    let Lower {
        module,
        bcx,
        global_side_trace_box,
        escape,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        mut current_kinds,
        mut dispatchable,
        mut dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        ..
    } = lower;
    let Plan {
        record,
        max_stack,
        window_size,
        effective_end,
        call_idx_opt,
        for_loop_idx_opt,
        inline_abort_idx_opt,
        return_idx_opt,
        ..
    } = *pl;
    let op_offsets = &pl.op_offsets;
    let head_live = &pl.head_live;

    bcx.finalize(module.target_config());
    drop_unused_block_params(&mut ctx.func);
    // `LUNA_TRACE_IR_DUMP=1` dumps the cranelift IR of every
    // compiled trace fn to stderr. Categorization + density-reduction
    // tool for layer-6 attribution (per-call IR op count is the gap).
    if std::env::var("LUNA_TRACE_IR_DUMP")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        eprintln!(
            "=== TRACE IR DUMP head_pc={} n_recorded_ops={} ===\n{}\n=== END ===",
            record.head_pc,
            record.ops.len(),
            ctx.func.display()
        );
    }
    // module finalization is the JIT-specific
    // wrapper's job (see [`try_compile_trace_with_options`]). The
    // generic body emits the function definition and stops at
    // `clear_context`; the JIT wrapper calls `finalize_definitions`
    // + `get_finalized_function`, patches `compiled.entry` with the
    // real fn pointer, and parks the module on the Vm's
    // `storage.trace_handles` Vec.
    // The AOT pipeline (luna-aot) calls `ObjectModule::finish` /
    // `ObjectProduct::emit` to produce a `.o` file instead, and
    // resolves the trace symbol at static link time.

    // Op::ForLoop at the tail writes R[A] (next loop var), R[A+1]
    // (decremented count), and R[A+3] (visible loop var copy) —
    // all Int per the 5.4+ count form. Op::TForLoop writes R[A+2]
    // = R[A+4] on continue (TForLoop tail emit; R[A+4] = Int gated
    // by the tag check).
    if let Some(for_loop_idx) = for_loop_idx_opt {
        let rop = &record.ops[for_loop_idx];
        let a = rop.inst.a() as usize;
        match rop.inst.op() {
            Op::ForLoop => {
                current_kinds[a] = RegKind::Int;
                current_kinds[a + 1] = RegKind::Int;
                current_kinds[a + 3] = RegKind::Int;
            }
            Op::TForLoop => {
                current_kinds[a + 2] = RegKind::Int;
            }
            _ => {}
        }
    }
    // Derive exit_tags from the kind tracker's final state. Slots
    // the trace never touched stay `Untouched` (dispatcher restores
    // the entry tag); slots the trace wrote take the writer's
    // determined kind. `current_kinds` propagates source kinds at
    // the Move op so the dispatcher doesn't need a deferred
    // entry-tag lookup.
    // dispatch heuristic: a `Op::Call`-truncated
    // trace whose body is too short to amortise the dispatcher's
    // marshal-in + transmute + restore overhead is a net loss vs the
    // interpreter (measured at ~1.8× slower on fib_28's ~7-op body).
    // Keep such traces cached (compile cost is paid) but pin
    // dispatchable=false unless the per-dispatch body is large
    // enough to win. `MIN_DISPATCHABLE_TRUNC_BODY_BASE` is tuned to fib's
    // 7-op body being just below the gate at depth=0.
    //
    // scale the gate down as `max_depth_used` grows:
    // each extra inline level amortises ~2 ops worth of marshal
    // overhead per dispatch (one dispatch processes the full
    // chain of depth+1 frames). Saturating-sub so deep traces
    // never miss-fire on the length gate.
    const MIN_DISPATCHABLE_TRUNC_BODY_BASE: usize = 20;
    // floor at 40 ops/dispatch (the empirical
    // dispatcher-overhead amortisation line: ~80ns per dispatch /
    // ~2ns per body op). The adaptive `BASE - depth*2`
    // formula alone could drop the gate to 0 at MAX_INLINE_DEPTH=16,
    // letting tiny-body inline traces dispatch and pay overhead
    // they can't amortise (binary_trees_d4 runs 0.73× without the
    // floor). The floor doesn't affect fib_28 (~112 ops body —
    // well above the floor) but bails the binary_trees
    // pathological case.
    const MIN_DISPATCHABLE_TRUNC_BODY_FLOOR: usize = 40;
    let max_depth_used = record
        .ops
        .iter()
        .map(|r| r.inline_depth as usize)
        .max()
        .unwrap_or(0);
    let adaptive = MIN_DISPATCHABLE_TRUNC_BODY_BASE.saturating_sub(max_depth_used * 2);
    let min_dispatchable_trunc_body = adaptive.max(MIN_DISPATCHABLE_TRUNC_BODY_FLOOR);
    // inline traces (per_exit_metas non-empty)
    // skip the length-gate. Each dispatch tears through multiple
    // inlined frames so body-length isn't a useful proxy for the
    // dispatcher's marshal overhead; the gate would dump fib's
    // ~8-op-by-the-time-MAX_DEPTH-hits prefix even though one
    // dispatch processes 4 recursion levels.
    //
    // sunk-alloc traces also skip the length-gate.
    // Skipping even a single `Heap::new_table()` per dispatch
    // dwarfs the marshal-in/out overhead on a 7-op body, so the
    // gate's conservative default is a net loss here.
    //
    // Closure-creating traces do NOT skip the
    // length-gate. Unlike sunk emit which avoids `Heap::new_table()`,
    // the Op::Closure helper still calls `Heap::new_closure_inline`
    // — emit replaces only the interp's match-arm dispatch +
    // frame plumbing for the 2-op `Closure + Return1` shape, which
    // is less than trace dispatch's marshal+enter overhead. Per-iter
    // dispatch of a tiny closure-constructor body is a net loss
    // (probe: `closure_no_upval_for_500k` mac measured 0.53× when
    // the gate was skipped). Closure traces only earn dispatch when
    // body length passes the gate organically.
    // InlineAbort traces close without materialising frames. The
    // interp can't resume at the inline-abort PC without the
    // matching CallFrames, so gate dispatch off.
    // Recorded before the length gate: the first reason is the one
    // kept, and side-trace wiring tells a trace that is unsafe to run
    // from one that is only too short to dispatch by it.
    if inline_abort_idx_opt.is_some() {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("InlineAbort-gate"));
    }
    if (call_idx_opt.is_some() || return_idx_opt.is_some())
        && effective_end < min_dispatchable_trunc_body
        && per_exit_inline_vec.is_empty()
        && sunk_alloc_seen == 0
    {
        dispatchable = false;
        dispatch_off_reason = dispatch_off_reason.or(Some("length-gate"));
    }

    // clean-tail `exit_tags` cover the caller window
    // only ([0..max_stack)). Per-side-exit `per_exit_tags` for inline
    // cmp sites carry the full `window_size` snapshot
    // because the dispatcher must restore EVERY pushed frame's
    // register window, not just the caller's.
    let mut exit_tags_vec = kinds_to_exit_tags(&current_kinds[..max_stack]);
    // for every sunk site at depth=0 (depth>0 is rejected
    // in pre-emit), force the slot's exit tag to `Untouched` so the
    // dispatcher carries the entry tag in the restore. Without this
    // override the slot's `current_kinds` could read as Table (from
    // some other path) or Unset, and the dispatcher would try to
    // unpack `reg_state[a]` (which we never wrote for sunk sites)
    // as a `Value::Table` of NULL bits → SIGSEGV.
    for site in &escape.sites {
        if site.state == EscapeState::Sinkable && site.inline_depth == 0 {
            let idx = site.a as usize;
            if idx < exit_tags_vec.len() {
                exit_tags_vec[idx] = ExitTag::Untouched;
            }
        }
    }
    let global_tag_res_kind = classify_exit_tags(&exit_tags_vec);
    let exit_tags: TArc<[ExitTag]> = exit_tags_vec.into();
    // split per_exit_kinds's 3-tuple into the
    // 2-tuple `per_exit_tags` for the dispatcher AND the parallel
    // `tags_side_trace_ptrs` Box slice the close handler writes to.
    // The Box transports the cell's heap address (baked into the
    // IR's `iconst` at each callsite) through this move without
    // moving the cell itself.
    let mut tags_side_boxes: Vec<Box<TCellPtr>> = Vec::with_capacity(per_exit_kinds.len());
    let per_exit_tags: TArc<[(u32, TArc<[ExitTag]>)]> = per_exit_kinds
        .into_iter()
        .map(|(pc, kinds, side_box)| {
            // The cmp emit site pushed the right slice
            // length (caller-window for depth=0, full window for
            // depth>0). Hand it through verbatim — the dispatcher
            // iterates `exit_tags_for_pc.len()` and walks both
            // shapes uniformly.
            let tags: TArc<[ExitTag]> = kinds_to_exit_tags(&kinds).into();
            tags_side_boxes.push(side_box);
            (pc, tags)
        })
        .collect::<Vec<_>>()
        .into();
    let tags_side_trace_ptrs: TArc<[Box<TCellPtr>]> = tags_side_boxes.into();
    let per_exit_inline: TArc<[InlineSideExit]> = per_exit_inline_vec
        .into_iter()
        .map(
            |(cont_pc, head_resume_pc, kinds, chain, side_trace_ptr)| InlineSideExit {
                cont_pc,
                head_resume_pc,
                exit_tags: kinds_to_exit_tags(&kinds).into(),
                chain,
                side_trace_ptr,
            },
        )
        .collect::<Vec<_>>()
        .into();

    checkpoint("post:emit-pass-done");
    // pre-compute exit_hit_counts before the struct
    // init so per_exit_tags's len is still accessible.
    let exit_hit_counts: TArc<[TCellU32]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellU32> = (0..total).map(|_| TCellU32::new(0)).collect();
        v.into()
    };
    // parallel per-exit raw fn-ptr slots, all null
    // until a child side trace compiles for the slot. Same length
    // as exit_hit_counts.
    let exit_side_trace_ptrs: TArc<[TCellPtr]> = {
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let v: Vec<TCellPtr> = (0..total).map(|_| TCellPtr::null()).collect();
        v.into()
    };
    let compiled = CompiledTrace {
        head_pc: record.head_pc,
        // caller (JIT wrapper or AOT pipeline)
        // patches `entry` after finalize. See [`placeholder_trace_fn`].
        entry: placeholder_trace_fn,
        n_ops: record.ops.len() as u32,
        dispatchable,
        // real window_size ≥ max_stack; the
        // dispatcher reads this to size its reg_state buffer.
        window_size,
        exit_tags,
        global_tag_res_kind,
        is_inline_abort_close: inline_abort_idx_opt.is_some(),
        dispatch_off_reason: if dispatchable {
            None
        } else {
            dispatch_off_reason
        },
        entry_tags: record
            .entry_tags
            .iter()
            .enumerate()
            .map(|(i, &t)| match head_live.get(i) {
                Some(false) => ENTRY_TAG_ANY,
                _ => t,
            })
            .collect::<Vec<u8>>()
            .into(),
        per_exit_tags,
        // populated by the cmp@d>0 emit sites
        // above; the IR encodes `(site_idx + 1)` in the upper 32
        // bits of its return value so the dispatcher can pull the
        // right entry. Holding the inner Rc<[FrameMaterializeInfo]>
        // alive keeps each chain's address stable across dispatches
        // (cranelift IR has the raw pointer baked in via iconst).
        exit_hit_counts,
        exit_side_trace_ptrs,
        // per-TAG-entry side-trace cells (parallel
        // to per_exit_tags) + the GLOBAL singleton cell. Both
        // collected from Boxes allocated AT each emit callsite so
        // the IR has baked the right heap address.
        tags_side_trace_ptrs,
        global_side_trace_ptr: global_side_trace_box,
        // empty at compile; close handler fills
        // it as child side traces compile for this trace's hot
        // exits.
        side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
        has_any_side_wired: TCellBool::new(false),
        per_exit_inline,
        // diagnostic only; counts Sinkable sites from the
        // pre-emit sweep. Vm sums these into
        // `trace_sinkable_seen_count`.
        sinkable_sites_seen: escape.sinkable_count(),
        accum_bufferable_seen: escape
            .accum_sites
            .iter()
            .filter(|s| s.state == BufferState::Bufferable)
            .count() as u32,
        // count of sites that actually took the sunk-emit
        // path in this trace's body (NewTable replaced by virt slot
        // Variables, no heap alloc helper called). Vm bumps
        // `trace_sunk_alloc_count` by this on compile success.
        sunk_alloc_seen,
        // count of materialise emit sites for sunk slot
        // recovery at cmp side-exits.
        materialize_emit_count,
        // count of Op::Closure ops the trace lowered.
        closure_seen,
        // compute body_writes for the smart side-trace
        // gate. Uses op_offsets (already computed above) to apply
        // inline-depth offsets per op.
        body_writes: compute_body_writes(record, &op_offsets).into(),
        // populated by the `downrec_idx_opt` arm
        // above into `downrec_link_for_compiled`. When the arm
        // emitted a stitch sentinel + caller-pc guard, this carries
        // `Some((0, head_pc))`; otherwise `None`. `Some(_)` alone
        // doesn't make the trace dispatchable — that takes a
        // multi-way candidate count >= 2 (see
        // `downrec_multi_way_count` below).
        downrec_link: downrec_link_for_compiled,
        // multi-way guard candidate count baked
        // into the IR's CMP-chain. `0` for non-DownRec closes;
        // `1` for single-CMP-fallback DownRec; `>= 2` for the
        // lifted `dispatchable = true` path.
        downrec_multi_way_count: downrec_multi_way_count_for_compiled,
    };
    // decided only now: the dispatch gates above run after the emit pass
    if always_codegen || trace_is_enterable(record, &compiled) {
        // `LUNA_TRACE_ASM_DUMP=1` requests cranelift to
        // emit the post-regalloc machine-code disassembly (vcode) and dumps
        // it to stderr after `define_function`. Used for the cargo-asm
        // decomposition of the table-field IC under env-OFF vs env-ON.
        let want_asm_dump = std::env::var("LUNA_TRACE_ASM_DUMP")
            .map(|v| v == "1")
            .unwrap_or(false);
        if want_asm_dump {
            ctx.set_disasm(true);
        }
        module.define_function(fn_id, &mut ctx).ok()?;
        if want_asm_dump
            && let Some(cc) = ctx.compiled_code()
            && let Some(vcode) = cc.vcode.as_ref()
        {
            eprintln!(
                "=== TRACE ASM DUMP head_pc={} n_recorded_ops={} ===\n{}\n=== END ===",
                record.head_pc,
                record.ops.len(),
                vcode
            );
        }
        module.clear_context(&mut ctx);
    }
    Some((fn_id, compiled))
}
