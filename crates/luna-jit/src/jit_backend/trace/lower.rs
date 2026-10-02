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

mod begin;
mod body;
mod downrec_tail;
mod exit;
mod finish;
mod fold;
mod helpers;
mod loop_tail;
mod ops;
mod plan;
mod prologue;
mod tail;
use begin::*;
use body::*;
use downrec_tail::*;
use exit::*;
use finish::*;
use fold::*;
use helpers::*;
use loop_tail::*;
use ops::*;
use plan::*;
use prologue::*;
use tail::*;

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
    let Plan { record, .. } = *pl;
    emit_fold_precheck(lw, pl);
    emit_body(lw, pl)?;
    let (downrec_link_for_compiled, downrec_multi_way_count_for_compiled) = emit_tail(lw, pl)?;
    let Lower {
        module,
        bcx,
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        escape,
        global_side_trace_box,
        ..
    } = lower;
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

    let compiled = build_compiled(
        pl,
        Emitted {
            current_kinds,
            dispatchable,
            dispatch_off_reason,
            per_exit_kinds,
            per_exit_inline_vec,
            sunk_alloc_seen,
            materialize_emit_count,
            closure_seen,
            escape,
            global_side_trace_box,
            downrec_link_for_compiled,
            downrec_multi_way_count_for_compiled,
        },
    );
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
