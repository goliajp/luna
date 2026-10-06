use super::*;

pub(super) fn lower_clif<M: Module>(
    module: &mut M,
    pl: &Plan<'_>,
    escape: EscapeAnalysis,
    aot_fn_name: Option<&str>,
    always_codegen: bool,
) -> Option<(FuncId, CompiledTrace)> {
    let Plan { record, .. } = *pl;
    let mut ctx = module.make_context();
    let mut fbc = FunctionBuilderContext::new();
    let b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let mut e = ClifEmit {
        b,
        m: module,
        relocs: Vec::new(),
    };
    let h = declare_helpers(&mut e)?;
    let mut sig = e.make_signature();
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
    let fn_id = e
        .declare_function(trace_fn_name, trace_fn_linkage, &sig)
        .ok()?;
    e.b.func.signature = sig;
    e.b.func.name = UserFuncName::user(0, fn_id.as_u32());

    let (e, emitted) = emit_trace(e, pl, h, escape, 0)?;
    let ClifEmit {
        b: bcx,
        m: module,
        relocs,
    } = e;
    bcx.finalize(module.target_config());
    drop_unused_block_params(&mut ctx.func);
    reloc::set_values(&relocs);
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

    let compiled = build_compiled(pl, emitted);
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
        super::code_dump::note_size(&ctx);
        reloc::note_sites(&*module, &ctx);
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
