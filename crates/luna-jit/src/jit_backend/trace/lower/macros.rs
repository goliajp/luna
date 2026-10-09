//! The guard macros the emit code uses.

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
        let fref = $lw.bcx.import_func($id);
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
