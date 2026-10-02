use super::*;

/// The checked table-store helpers, by the key's kind.
pub(super) struct StoreHelpers {
    pub(super) int_key: cranelift_module::FuncId,
    pub(super) str_key: cranelift_module::FuncId,
    pub(super) any_key: cranelift_module::FuncId,
}

/// Calls the checked table store `t[key] = v` for a key and a value of
/// known kinds, returning the helper's status (`1` stored, `0` the
/// interpreter must do it); the caller side-exits on `0`. Integer and
/// string keys have their own helpers, which need not rebuild the key
/// from a tag.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_table_set<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
    helpers: &StoreHelpers,
    t: Value,
    key: Value,
    key_kind: RegKind,
    val: Value,
    val_kind: RegKind,
) -> Value {
    let val_tag = bcx.ins().iconst(types::I64, i64::from(kind_tag(val_kind)));
    let call = match key_kind {
        RegKind::Int | RegKind::Str => {
            let id = if key_kind == RegKind::Int {
                helpers.int_key
            } else {
                helpers.str_key
            };
            let f = module.declare_func_in_func(id, bcx.func);
            bcx.ins().call(f, &[t, key, val, val_tag])
        }
        _ => {
            let key_tag = bcx.ins().iconst(types::I64, i64::from(kind_tag(key_kind)));
            let f = module.declare_func_in_func(helpers.any_key, bcx.func);
            bcx.ins().call(f, &[t, key, key_tag, val, val_tag])
        }
    };
    bcx.inst_results(call)[0]
}

/// flush context for the buffered string
/// accumulator emit. When `Some`, both
/// `emit_store_back_and_return_*` emit a `luna_jit_str_buf_intern`
/// + `def_var(accum_slot, str_ptr)` + `luna_jit_str_buf_release`
/// sequence BEFORE the existing store-back loop, so the
/// accumulator slot holds a real LuaStr ptr by the time the
/// dispatcher restores from reg_state.
#[derive(Clone, Copy)]
pub(super) struct FlushCtx {
    pub(super) buf_var: Variable,
    pub(super) accum_slot: u32,
    pub(super) intern_ref: cranelift_codegen::ir::FuncRef,
    pub(super) release_ref: cranelift_codegen::ir::FuncRef,
}

/// depth-relative base address helper.
///
/// Given the trace's `base_var` Variable (declared at entry block in
/// `lower_trace_into_named`) plus an op's window
/// offset (`op_offset_bytes` = `op_offsets[i] * 8`) plus a slot index
/// within that op's window, returns a `(base_value, byte_offset)`
/// pair suitable for `bcx.ins().load(..., base_value, byte_offset)`
/// or `bcx.ins().store(..., base_value, byte_offset)`.
///
/// Caller contract: `base_var` is initialised to `iconst(0)` —
/// i.e., a depth-0 sentinel placeholder. Calling this helper produces
/// load/store IR that addresses `[0 + op_offset_bytes + slot * 8]`,
/// which is NOT a valid reg_state-relative address. Op-arms
/// MUST NOT call this helper (they keep using `regs_full[off + slot]`
/// via `bcx.use_var` / `bcx.def_var`).
///
/// Why a helper (not inline `iadd_imm` at each call site): the
/// op_offset + slot arithmetic is identical across all op-arms and
/// LuaJIT's own sources show arm64's `ldr Xd, [Xn, #imm]` handles the pattern
/// in a single addressing mode. Concentrating the math in one helper
/// keeps the Cranelift mid-end's `iadd_imm` coalescing surface
/// uniform and keeps codegen auditable at ONE site instead of ~30.
// No production caller yet; the only consumer is the regression test
// `base_var_scaffold.rs`. `pub(crate)` so the test crate's
// hook can dispatch through `try_compile_trace_with_options`.
#[allow(dead_code)]
pub(crate) fn current_base_addr(
    bcx: &mut FunctionBuilder<'_>,
    base_var: Variable,
    op_offset_bytes: i32,
    slot: u32,
) -> (Value, i32) {
    let base_now = bcx.use_var(base_var);
    let total_offset = op_offset_bytes.saturating_add((slot as i32).saturating_mul(8));
    (base_now, total_offset)
}

pub(super) fn emit_flush_buf(bcx: &mut FunctionBuilder<'_>, ctx: &FlushCtx, regs: &[Variable]) {
    let buf_ptr = bcx.use_var(ctx.buf_var);
    let call_inst = bcx.ins().call(ctx.intern_ref, &[buf_ptr]);
    let str_ptr = bcx.inst_results(call_inst)[0];
    if let Some(&accum_var) = regs.get(ctx.accum_slot as usize) {
        bcx.def_var(accum_var, str_ptr);
    }
    bcx.ins().call(ctx.release_ref, &[buf_ptr]);
}

/// emit the indirect-call-or-return gate. Loads
/// the cell at `side_trace_cell_addr`; if non-null, tail-calls the
/// child fn via `call_indirect`, ORs sentinel bits 56..=63 into the
/// child's i64 return, and returns. Otherwise runs the caller's
/// `normal_return` closure. The caller must have ALREADY written the
/// reg_state slots before calling this (the child reads them via its
/// entry block).
pub(super) fn emit_side_trace_or_return(
    bcx: &mut FunctionBuilder<'_>,
    reg_state: Value,
    side_trace_cell_addr: i64,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    sentinel_code: u32,
    normal_return: impl FnOnce(&mut FunctionBuilder<'_>),
) {
    // `side_trace_cell_addr == 0` is the "no-gate"
    // sentinel: emit the normal return only, skipping the load +
    // icmp + brif + call_indirect IR. The gate is a net perf loss
    // when it fires at every dispatch site (19 callsites × `load +
    // icmp + brif` per parent dispatch > amortization from rare
    // side-trace fires), so only the TAG callsites (3), where hot
    // exits actually live, get a cell; the 14 GLOBAL + 2 INLINE
    // callsites pass 0 here and avoid the overhead. The
    // close-handler still writes child entry ptrs to the legacy `exit_side_trace_ptrs` + the per-kind cells so the
    // counters stay populated; only the IR gate emission is gated.
    if side_trace_cell_addr == 0 {
        normal_return(bcx);
        return;
    }
    let cell_addr = bcx.ins().iconst(types::I64, side_trace_cell_addr);
    let fn_ptr = bcx
        .ins()
        .load(types::I64, MemFlagsData::trusted(), cell_addr, 0);
    let null = bcx.ins().iconst(types::I64, 0);
    let has_side = bcx.ins().icmp(IntCC::NotEqual, fn_ptr, null);
    let do_side_blk = bcx.create_block();
    let do_exit_blk = bcx.create_block();
    bcx.ins().brif(has_side, do_side_blk, &[], do_exit_blk, &[]);

    // Side block: indirect call into the child trace. ABI matches
    // the parent's own (`(I64) -> I64`); pass the same reg_state
    // pointer so the child sees the just-stored slots. OR sentinel
    // bits 56..=63 into the child's i64 return so the dispatcher
    // re-decodes via the SIDE TRACE's shape inputs.
    bcx.switch_to_block(do_side_blk);
    bcx.seal_block(do_side_blk);
    let call_inst = bcx
        .ins()
        .call_indirect(trace_fn_sig_ref, fn_ptr, &[reg_state]);
    let body = bcx.inst_results(call_inst)[0];
    let mask_u64: u64 = (1u64 << 63) | ((sentinel_code as u64 & 0x7F) << 56);
    let mask_v = bcx.ins().iconst(types::I64, mask_u64 as i64);
    let masked = bcx.ins().bor(body, mask_v);
    bcx.ins().return_(&[masked]);

    // Exit block: caller-provided normal return path (plain
    // encoded-return semantics).
    bcx.switch_to_block(do_exit_blk);
    bcx.seal_block(do_exit_blk);
    normal_return(bcx);
}

pub(super) fn emit_store_back_and_return_pc(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    stored: &[Option<Value>],
    reg_state: Value,
    pc: u32,
    flush_ctx: Option<&FlushCtx>,
    side_trace_cell_addr: i64,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    sentinel_code: u32,
) {
    emit_store_back_and_return(
        bcx,
        regs,
        stored,
        reg_state,
        i64::from(pc),
        flush_ctx,
        side_trace_cell_addr,
        trace_fn_sig_ref,
        sentinel_code,
    );
}

/// [`emit_store_back_and_return_pc`] returning `ret`, an encoded exit
/// (see `decode_exit_shape`), instead of a bare pc.
pub(super) fn emit_store_back_and_return(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    stored: &[Option<Value>],
    reg_state: Value,
    ret: i64,
    flush_ctx: Option<&FlushCtx>,
    side_trace_cell_addr: i64,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    sentinel_code: u32,
) {
    if let Some(ctx) = flush_ctx {
        emit_flush_buf(bcx, ctx, regs);
    }
    // reg_state already holds `stored[idx]` (see `sync_reg_state`); only
    // a register whose value changed since is written.
    for (idx, v) in regs.iter().copied().enumerate() {
        let val = bcx.use_var(v);
        if stored.get(idx).copied().flatten() == Some(val) {
            continue;
        }
        let offset = (idx as i32) * 8;
        bcx.ins().store(MemFlagsData::new(), val, reg_state, offset);
    }
    emit_side_trace_or_return(
        bcx,
        reg_state,
        side_trace_cell_addr,
        trace_fn_sig_ref,
        sentinel_code,
        |bcx| {
            let ret_val = bcx.ins().iconst(types::I64, ret);
            bcx.ins().return_(&[ret_val]);
        },
    );
}

/// Writes every register whose SSA value differs from what reg_state
/// holds (`stored`) and records the new values. Called at the start of
/// each recorded op and before every back-edge, it keeps reg_state equal
/// to the registers as of the last completed op, so an exit stores only
/// what the op it leaves from changed (usually nothing) instead of the
/// whole window. That made every exit a block of stores, which is what
/// the trace's compile time scaled with.
pub(super) fn sync_reg_state(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    stored: &mut [Option<Value>],
    reg_state: Value,
) {
    for (idx, v) in regs.iter().copied().enumerate() {
        let val = bcx.use_var(v);
        if stored[idx] == Some(val) {
            continue;
        }
        bcx.ins()
            .store(MemFlagsData::new(), val, reg_state, (idx as i32) * 8);
        stored[idx] = Some(val);
    }
}

/// A depth-0 side exit restoring through `per_exit_tags[tags_idx]`: the
/// return value names the snapshot, since exits resuming at the same pc
/// can carry different register kinds (see `decode_exit_shape`). An exit
/// to the trace's own head has not run the head op, so it also stops the
/// dispatcher from re-entering the trace before the interpreter has.
pub(super) fn emit_tagged_exit<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
    suppress_admit_id: cranelift_module::FuncId,
    regs: &[Variable],
    stored: &[Option<Value>],
    reg_state: Value,
    pc: u32,
    head_pc: u32,
    tags_idx: u32,
    flush_ctx: Option<&FlushCtx>,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
) {
    if pc == head_pc {
        let r = module.declare_func_in_func(suppress_admit_id, bcx.func);
        bcx.ins().call(r, &[]);
    }
    let ret = luna_core::jit::trace_types::EXIT_TAGS_INDEX_BIT
        | (u64::from(tags_idx) << 32)
        | u64::from(pc);
    emit_store_back_and_return(
        bcx,
        regs,
        stored,
        reg_state,
        ret as i64,
        flush_ctx,
        0i64,
        trace_fn_sig_ref,
        encode_side_sentinel(SIDE_SENT_KIND_TAG, tags_idx),
    );
}

/// The resume pc of a trace's return value, whichever exit encoding it
/// uses (see `decode_exit_shape`).
#[cfg(test)]
pub(crate) fn exit_pc(ret: i64) -> i64 {
    ret & 0xFFFF_FFFF
}

/// inline cmp@d>0 side-exit return shape. The
/// upper 32 bits encode `site_idx + 1` (1-based; 0 means "no
/// inline site, look up via cont_pc in `per_exit_tags`"); the lower
/// 32 bits hold the resume PC. The dispatcher decodes this so a
/// cont_pc shared across multiple inline cmps (fib has 4+ such
/// sites colliding on pc=3) maps to the right entry's exit_tags
/// and chain.
pub(super) fn emit_store_back_and_return_site(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    stored: &[Option<Value>],
    reg_state: Value,
    site_idx: u32,
    cont_pc: u32,
    flush_ctx: Option<&FlushCtx>,
    side_trace_cell_addr: i64,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
) {
    if let Some(ctx) = flush_ctx {
        emit_flush_buf(bcx, ctx, regs);
    }
    // reg_state already holds `stored[idx]` (see `sync_reg_state`); only
    // a register whose value changed since is written.
    for (idx, v) in regs.iter().copied().enumerate() {
        let val = bcx.use_var(v);
        if stored.get(idx).copied().flatten() == Some(val) {
            continue;
        }
        let offset = (idx as i32) * 8;
        bcx.ins().store(MemFlagsData::new(), val, reg_state, offset);
    }
    let sentinel = encode_side_sentinel(SIDE_SENT_KIND_INLINE, site_idx);
    emit_side_trace_or_return(
        bcx,
        reg_state,
        side_trace_cell_addr,
        trace_fn_sig_ref,
        sentinel,
        |bcx| {
            let encoded = (((site_idx as u64) + 1) << 32) | (cont_pc as u64);
            let v = bcx.ins().iconst(types::I64, encoded as i64);
            bcx.ins().return_(&[v]);
        },
    );
}
