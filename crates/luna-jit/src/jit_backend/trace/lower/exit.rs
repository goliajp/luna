use super::*;

// A guard that fails leaves the trace at `$pc` (the op being
// guarded, re-executed by the interpreter) exactly as a cmp side
// exit does: live sunk tables are materialised and, when the op sits
// in an inlined frame, the frames are rebuilt first. An exit to the
// head also stops the dispatcher from entering the trace again before
// the interpreter has run the head op (see `emit_tagged_exit`).
pub(super) fn guard_exit<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, pc: u32, i: usize) {
    let Plan {
        record,
        head_proto,
        max_stack,
        opts,
        window_size_us,
        ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id,
        materialize_id,
        mat_sunk_id,
        ..
    } = lw.h.rt;
    let side_exit_pc: u32 = pc;
    if !lw.call_chain.is_empty() {
        let head_resume_pc = lw.call_chain[0].pc;
        let mut snapshot: Vec<FrameMaterializeInfo> = lw.call_chain.clone();
        if let Some(last) = snapshot.last_mut() {
            last.pc = side_exit_pc;
        }
        let chain_rc: TArc<[FrameMaterializeInfo]> = snapshot.into();
        let chain_ptr = TArc::as_ptr(&chain_rc) as *const FrameMaterializeInfo as i64;
        let chain_len = chain_rc.len() as i64;
        let site_idx = lw.per_exit_inline_vec.len() as u32;
        let mut kinds_snapshot: Vec<RegKind> = lw.current_kinds.clone();
        let mat_count = emit_materialize_live_sunk(
            &mut lw.bcx,
            mat_sunk_id,
            &lw.escape,
            &lw.virt_vars,
            &lw.virt_kinds,
            &lw.regs_full,
            &pl.op_offsets,
            i,
            &mut kinds_snapshot,
            head_proto,
            opts.aot,
            &mut lw.defined_aot_data,
        );
        lw.materialize_emit_count += mat_count;
        let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
        let chain_for_helper = chain_rc.clone();
        lw.per_exit_inline_vec.push((
            side_exit_pc,
            head_resume_pc,
            kinds_snapshot,
            chain_rc,
            side_box,
        ));
        let n_arg = lw.bcx.ins().iconst(types::I64, chain_len);
        let ptr_arg = emit_chain_ptr_arg(
            &mut lw.bcx,
            &chain_for_helper,
            chain_ptr,
            opts.aot,
            &mut lw.defined_aot_data,
        );
        let mat_ref = lw.bcx.import_func(materialize_id);
        let _ = lw.bcx.ins().call(mat_ref, &[n_arg, ptr_arg]);
        emit_store_back_and_return_site(
            &mut lw.bcx,
            &lw.regs_full[..window_size_us],
            &lw.stored,
            reg_state,
            site_idx,
            side_exit_pc,
            lw.flush_ctx.as_ref(),
            0i64,
            trace_fn_sig_ref,
        );
    } else {
        let mut snapshot: Vec<RegKind> = lw.current_kinds[..max_stack].to_vec();
        let mat_count = emit_materialize_live_sunk(
            &mut lw.bcx,
            mat_sunk_id,
            &lw.escape,
            &lw.virt_vars,
            &lw.virt_kinds,
            &lw.regs_full,
            &pl.op_offsets,
            i,
            &mut snapshot,
            head_proto,
            opts.aot,
            &mut lw.defined_aot_data,
        );
        lw.materialize_emit_count += mat_count;
        let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
        let tag_side_local = lw.per_exit_kinds.len() as u32;
        lw.per_exit_kinds.push((side_exit_pc, snapshot, side_box));
        emit_tagged_exit(
            &mut lw.bcx,
            suppress_admit_id,
            &lw.regs_full[..max_stack],
            &lw.stored,
            reg_state,
            side_exit_pc,
            record.head_pc,
            tag_side_local,
            lw.flush_ctx.as_ref(),
            trace_fn_sig_ref,
        );
    }
}
