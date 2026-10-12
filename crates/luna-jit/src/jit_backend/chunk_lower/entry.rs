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
}

/// How a self-recursive chunk's ring is built and checked.
#[derive(Clone, Copy)]
pub(super) struct RingSpec {
    /// `luna_jit_helpers::self_call_desc` of the self calls
    pub(super) desc: i64,
    /// the dialect counts calls against a depth limit (5.1, and 5.2,
    /// whose budget is unbounded, compiled alike)
    pub(super) counted: bool,
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
    let call = bcx.ins().call(callee_ref, &args);
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

/// Defines the stub the last copy of a self-recursive body calls (and the
/// entry calls first): with the native stack below the limit in its
/// context, or fewer calls left than the ring has copies, it has
/// `luna_jit_self_call_slow` make the call in the interpreter; otherwise
/// it takes the ring's calls out of the budget and calls the first copy.
fn define_stub<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    body_id: FuncId,
    num_params: usize,
    ring: RingSpec,
) -> Option<FuncId> {
    let mut sig = module.make_signature();
    for _ in 0..num_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let stub_id = module
        .declare_function("luna_jit_chunk_stub", Linkage::Local, &sig)
        .ok()?;
    let mut slow_sig = module.make_signature();
    for _ in 0..7 {
        slow_sig.params.push(AbiParam::new(types::I64));
    }
    slow_sig.returns.push(AbiParam::new(types::I64));
    let slow_id = module
        .declare_function("luna_jit_self_call_slow", Linkage::Import, &slow_sig)
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
    let mut go_slow = bcx.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
    let copies = i64::from(ring.copies);
    let left = ring
        .counted
        .then(|| bcx.ins().load(types::I64, flags, ctx_addr, 16));
    if let Some(left) = left {
        let spent = bcx
            .ins()
            .icmp_imm_s(IntCC::SignedLessThanOrEqual, left, copies);
        go_slow = bcx.ins().bor(go_slow, spent);
    }
    bcx.ins().brif(go_slow, slow, &[], fast, &[]);

    bcx.switch_to_block(fast);
    if let Some(left) = left {
        let fewer = bcx.ins().iadd_imm_s(left, -copies);
        bcx.ins().store(flags, fewer, ctx_addr, 16);
    }
    let body_ref = module.declare_func_in_func(body_id, bcx.func);
    let call = bcx.ins().call(body_ref, &args);
    let r = bcx.inst_results(call)[0];
    if let Some(left) = left {
        bcx.ins().store(flags, left, ctx_addr, 16);
    }
    bcx.ins().return_(&[r]);

    bcx.switch_to_block(slow);
    let slow_ref = module.declare_func_in_func(slow_id, bcx.func);
    let desc = bcx.ins().iconst(types::I64, ring.desc);
    let left = left.unwrap_or_else(|| {
        bcx.ins()
            .iconst(types::I64, luna_jit_helpers::SELF_CALL_UNCOUNTED)
    });
    let mut slow_args = vec![ctx_addr, desc, left];
    slow_args.extend_from_slice(&args);
    slow_args.resize_with(7, || bcx.ins().iconst(types::I64, 0));
    let call = bcx.ins().call(slow_ref, &slow_args);
    let r = bcx.inst_results(call)[0];
    bcx.ins().return_(&[r]);

    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());
    module.define_function(stub_id, ctx).ok()?;
    chunk_share::note(module, ctx, stub_id);
    module.clear_context(ctx);
    chunk_share::note_ring(body_id, stub_id, ring.copies);
    Some(stub_id)
}
