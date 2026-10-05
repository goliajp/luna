use super::*;

/// What a chunk's entry verifies before running the body.
pub(super) struct EntryChecks {
    /// Upvalue the self-recursive calls go through.
    pub(super) self_upval: Option<u32>,
    /// `("math", name)` key pairs of the folded `math.<name>` calls.
    pub(super) math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)>,
    /// The body calls itself, taking these [`SelfCalls`] parameters.
    pub(super) self_calls: Option<SelfCallParams>,
}

/// Defines the entry that runs `checks` before calling the chunk body
/// `body_id`: when one fails it returns at once with a deopt parked, and
/// the dispatcher runs the call in the interpreter.
pub(super) fn define_checked_entry<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    body_id: FuncId,
    checks: &EntryChecks,
    num_params: usize,
) -> Option<FuncId> {
    let mut sig = module.make_signature();
    for _ in 0..num_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let entry_id = module
        .declare_function("luna_jit_chunk_entry", Linkage::Local, &sig)
        .ok()?;
    let mut self_sig = module.make_signature();
    self_sig.params.push(AbiParam::new(types::I64));
    self_sig.returns.push(AbiParam::new(types::I64));
    let self_check_id = module
        .declare_function("luna_jit_self_upval_check", Linkage::Import, &self_sig)
        .ok()?;
    let mut math_sig = module.make_signature();
    math_sig.params.push(AbiParam::new(types::I64));
    math_sig.params.push(AbiParam::new(types::I64));
    math_sig.returns.push(AbiParam::new(types::I64));
    let math_check_id = module
        .declare_function("luna_jit_math_fn_is_library", Linkage::Import, &math_sig)
        .ok()?;
    let park_id = module
        .declare_function(
            "luna_jit_park_deopt",
            Linkage::Import,
            &module.make_signature(),
        )
        .ok()?;

    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, entry_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let entry = bcx.create_block();
    let bail = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let args: Vec<Value> = bcx.block_params(entry).to_vec();
    // luna_jit_self_upval_check parks its own deopt; the math check
    // leaves that to the bail block.
    let park_on_bail = !checks.math_fns.is_empty();
    if let Some(idx) = checks.self_upval {
        let check_ref = module.declare_func_in_func(self_check_id, bcx.func);
        let idx = bcx.ins().iconst(types::I64, i64::from(idx));
        let call = bcx.ins().call(check_ref, &[idx]);
        let ok = bcx.inst_results(call)[0];
        let next = bcx.create_block();
        bcx.ins().brif(ok, next, &[], bail, &[]);
        bcx.switch_to_block(next);
    }
    for &(math_key, name_key) in &checks.math_fns {
        let check_ref = module.declare_func_in_func(math_check_id, bcx.func);
        let m = chunk_share::str_arg(module, &mut bcx, math_key);
        let k = chunk_share::str_arg(module, &mut bcx, name_key);
        let call = bcx.ins().call(check_ref, &[m, k]);
        let ok = bcx.inst_results(call)[0];
        let next = bcx.create_block();
        bcx.ins().brif(ok, next, &[], bail, &[]);
        bcx.switch_to_block(next);
    }
    let mut args = args;
    let mut saved_pinned = None;
    if let Some(extra) = checks.self_calls {
        let fill_id = module
            .declare_function("luna_jit_enter_ctx", Linkage::Import, &{
                let mut s = module.make_signature();
                s.params.push(AbiParam::new(types::I64));
                s
            })
            .ok()?;
        let fill_ref = module.declare_func_in_func(fill_id, bcx.func);
        let bytes = 8 * luna_jit_helpers::SELF_CTX_WORDS as u32;
        let slot =
            bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3));
        let ctx_addr = bcx.ins().stack_addr(types::I64, slot, 0);
        bcx.ins().call(fill_ref, &[ctx_addr]);
        let limit = bcx.ins().stack_load(types::I64, types::I64, slot, 0);
        // the caller's value of the register, which the ABI makes ours to keep
        saved_pinned = Some(bcx.ins().get_pinned_reg(types::I64));
        bcx.ins().set_pinned_reg(limit);
        if extra.count {
            args.push(bcx.ins().stack_load(types::I64, types::I64, slot, 16));
        }
        if extra.ctx {
            args.push(ctx_addr);
        }
    }
    let body_ref = module.declare_func_in_func(body_id, bcx.func);
    let call = bcx.ins().call(body_ref, &args);
    let r = bcx.inst_results(call)[0];
    if let Some(saved) = saved_pinned {
        bcx.ins().set_pinned_reg(saved);
    }
    bcx.ins().return_(&[r]);

    bcx.switch_to_block(bail);
    if park_on_bail {
        let park_ref = module.declare_func_in_func(park_id, bcx.func);
        bcx.ins().call(park_ref, &[]);
    }
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());
    module.define_function(entry_id, ctx).ok()?;
    chunk_share::note(module, ctx, entry_id);
    module.clear_context(ctx);
    Some(entry_id)
}
