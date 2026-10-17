use super::*;

/// What a chunk's entry verifies before running the body.
pub(super) struct EntryChecks {
    /// Upvalue the self-recursive calls go through.
    pub(super) self_upval: Option<u32>,
    /// `("math", name)` key pairs of the folded `math.<name>` calls.
    pub(super) math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)>,
    /// The body calls itself: it runs as a ring of copies behind a stub
    /// (see [`SelfCalls`]).
    pub(super) ring: Option<RingSpec>,
    /// The tag of each argument, for the interpreter when a check fails.
    pub(super) arg_tags: Vec<u8>,
}

/// How a self-recursive chunk's ring is built and checked.
#[derive(Clone, Copy)]
pub(super) struct RingSpec {
    /// copies of the body in the ring: levels between two checks
    pub(super) copies: u32,
}

/// Defines the entry that runs `checks` before calling the chunk body
/// `body_id`: when one fails it returns at once with a deopt parked, and
/// the dispatcher runs the call in the interpreter. A self-recursive body
/// is called through its stub, with the stub's context filled and its
/// address in the pinned register.
pub(super) fn define_checked_entry<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    body_id: FuncId,
    checks: &EntryChecks,
    num_params: usize,
) -> Option<FuncId> {
    let callee = match checks.ring {
        Some(ring) => define_stub(module, ctx, body_id, num_params, ring)?,
        None => body_id,
    };
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

    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, entry_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let entry = bcx.create_block();
    let bail = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let args: Vec<Value> = bcx.block_params(entry).to_vec();
    let entry_args = args.clone();
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
    let mut saved_pinned = None;
    if checks.ring.is_some() {
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
        // the caller's value of the register, which the ABI makes ours to keep
        saved_pinned = Some(bcx.ins().get_pinned_reg(types::I64));
        bcx.ins().set_pinned_reg(ctx_addr);
    }
    let callee_ref = module.declare_func_in_func(callee, bcx.func);
    let mut args = args;
    if let Some(ctx_addr) = saved_pinned.map(|_| bcx.ins().get_pinned_reg(types::I64)) {
        // level 0: the base the call was made at, depth 0
        let base = bcx
            .ins()
            .load(types::I64, MemFlagsData::trusted(), ctx_addr, 24);
        args.push(base);
        args.push(bcx.ins().iconst(types::I64, 0));
    }
    let call = bcx.ins().call(callee_ref, &args);
    let r = bcx.inst_results(call)[0];
    if let Some(saved) = saved_pinned {
        bcx.ins().set_pinned_reg(saved);
    }
    bcx.ins().return_(&[r]);

    // a check failed: the interpreter runs the call from its start, the
    // level counted already
    bcx.switch_to_block(bail);
    let n = checks.arg_tags.len();
    let slot = bcx.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        (16 * n.max(1)) as u32,
        3,
    ));
    for (i, &tag) in checks.arg_tags.iter().enumerate() {
        let t = bcx.ins().iconst(types::I64, i64::from(tag));
        bcx.ins().stack_store(types::I64, t, slot, (16 * i) as i32);
        bcx.ins()
            .stack_store(types::I64, entry_args[i], slot, (16 * i + 8) as i32);
    }
    let mut hand_sig = module.make_signature();
    for _ in 0..4 {
        hand_sig.params.push(AbiParam::new(types::I64));
    }
    let hand_id = module
        .declare_function("luna_jit_handoff_frame", Linkage::Import, &hand_sig)
        .ok()?;
    let hand_ref = module.declare_func_in_func(hand_id, bcx.func);
    let pc0 = bcx.ins().iconst(types::I64, 0);
    let level0 = bcx.ins().iconst(types::I64, -1);
    let regs = bcx.ins().stack_addr(types::I64, slot, 0);
    let count = bcx.ins().iconst(types::I64, n as i64);
    bcx.ins().call(hand_ref, &[pc0, level0, regs, count]);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());
    module.define_function(entry_id, ctx).ok()?;
    chunk_share::note(module, ctx, entry_id);
    module.clear_context(ctx);
    Some(entry_id)
}

/// Defines the stub the last copy of a self-recursive body calls (and the
/// entry calls first): with the native stack below the limit in its
/// context, it leaves the call to the interpreter (the caller hands itself
/// over); otherwise it calls the first copy.
fn define_stub<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    body_id: FuncId,
    num_params: usize,
    ring: RingSpec,
) -> Option<FuncId> {
    let mut sig = module.make_signature();
    for _ in 0..num_params + 2 {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let stub_id = module
        .declare_function("luna_jit_chunk_stub", Linkage::Local, &sig)
        .ok()?;
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, stub_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let entry = bcx.create_block();
    let fast = bcx.create_block();
    let slow = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let args: Vec<Value> = bcx.block_params(entry).to_vec();
    let flags = MemFlagsData::trusted();
    let ctx_addr = bcx.ins().get_pinned_reg(types::I64);
    let limit = bcx.ins().load(types::I64, flags, ctx_addr, 0);
    let sp = bcx.ins().get_stack_pointer(types::I64);
    let go_slow = bcx.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
    bcx.ins().brif(go_slow, slow, &[], fast, &[]);

    bcx.switch_to_block(fast);
    let body_ref = module.declare_func_in_func(body_id, bcx.func);
    let call = bcx.ins().call(body_ref, &args);
    let r = bcx.inst_results(call)[0];
    bcx.ins().return_(&[r]);

    bcx.switch_to_block(slow);
    // the interpreter makes the call
    let not_started = bcx
        .ins()
        .iconst(types::I64, luna_jit_helpers::SELF_NOT_STARTED);
    bcx.ins().store(flags, not_started, ctx_addr, 8);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());
    module.define_function(stub_id, ctx).ok()?;
    chunk_share::note(module, ctx, stub_id);
    module.clear_context(ctx);
    chunk_share::note_ring(body_id, stub_id, ring.copies);
    Some(stub_id)
}

/// A self-recursive body's level, at `base` and `depth`, counted when it
/// reaches the limits in the context (`luna_jit_self_level`); the body
/// returns at once when that fails. Leaves the builder in the block that
/// goes on.
pub(super) fn count_level<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    desc: i64,
    base: Value,
    depth: Value,
) -> Option<()> {
    let mut sig = module.make_signature();
    for _ in 0..4 {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let level_id = module
        .declare_function("luna_jit_self_level", Linkage::Import, &sig)
        .ok()?;
    let flags = MemFlagsData::trusted();
    let ctx_addr = bcx.ins().get_pinned_reg(types::I64);
    let base_limit = bcx.ins().load(types::I64, flags, ctx_addr, 16);
    let cross = bcx
        .ins()
        .icmp(IntCC::SignedGreaterThanOrEqual, base, base_limit);
    let cold = bcx.create_block();
    let go_on = bcx.create_block();
    let failed = bcx.create_block();
    bcx.set_cold_block(cold);
    bcx.set_cold_block(failed);
    bcx.ins().brif(cross, cold, &[], go_on, &[]);
    bcx.switch_to_block(cold);
    let level_ref = module.declare_func_in_func(level_id, bcx.func);
    let desc = bcx.ins().iconst(types::I64, desc);
    let call = bcx.ins().call(level_ref, &[ctx_addr, desc, base, depth]);
    let r = bcx.inst_results(call)[0];
    bcx.ins().brif(r, failed, &[], go_on, &[]);
    bcx.switch_to_block(failed);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);
    bcx.switch_to_block(go_on);
    Some(())
}
