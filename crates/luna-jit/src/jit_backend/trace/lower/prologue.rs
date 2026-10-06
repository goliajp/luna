use super::*;

/// The entry block and the trace's registers, as [`emit_entry`] leaves them.
pub(super) struct EntryRegs {
    pub(super) reg_state: Value,
    pub(super) trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    pub(super) global_side_trace_box: Box<TCellPtr>,
    pub(super) regs_full: Vec<Variable>,
    pub(super) tforcall_tag_var: Variable,
    pub(super) tforcall_val_tag_var: Variable,
}

/// Emits the entry block: loads the caller window from `reg_state`,
/// zeroes the inline frames' registers and declares the trace-wide
/// variables.
pub(super) fn emit_entry<E: Emit>(bcx: &mut E, pl: &Plan<'_>) -> EntryRegs {
    let Plan {
        max_stack,
        window_size_us,
        ..
    } = *pl;
    // Two-block layout for the trace body:
    //
    // - `entry` is the function entry — receives the `reg_state`
    //   pointer as block param 0, loads each Lua reg from memory
    //   into a cranelift Variable, then unconditionally jumps to
    //   `body_loop`. The reg-load prelude runs *once* per
    //   dispatcher entry.
    // - `body_loop` is the loop head. The recorded op IR emits
    //   into it (or into successor blocks split off by cmp brifs).
    //   At the trace's clean close — when no `Op::Call` has
    //   truncated it — the tail emits a jump *back* to `body_loop`,
    //   so subsequent iterations stay inside the JIT'd code until
    //   a cmp side-exits. The dispatcher's per-iter marshal
    //   overhead amortizes across however many iterations the
    //   trace runs internally.
    //
    // Cranelift `FunctionBuilder` handles the back-edge phis
    // automatically: every reg's Variable gets a phi at
    // `body_loop`'s entry merging the entry-from-`entry` def
    // (initial load) with the loop-back def (the previous
    // iteration's writes). We delay sealing `body_loop` until
    // after the tail emits its back-edge so cranelift knows both
    // predecessors.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    bcx.seal_block(entry);
    let reg_state = bcx.block_params(entry)[0];

    // import the `TraceFn` ABI signature once so
    // every side-exit emit can `call_indirect` into a child side
    // trace. Matches the parent's own signature (`(I64) -> I64`).
    let trace_fn_sig_ref: cranelift_codegen::ir::SigRef = {
        let mut sig = bcx.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        bcx.import_signature(sig)
    };
    // singleton GLOBAL side-trace cell shared by
    // every non-INLINE / non-TAG callsite (clean-tail, Call
    // truncation, ForLoop / TForLoop exits, generic deopts). Each
    // such callsite bakes this Box's heap address into its IR.
    // Transported into [`CompiledTrace::global_side_trace_ptr`] at
    // emit end without moving (Box's heap allocation stays put).
    let global_side_trace_box: Box<TCellPtr> = Box::new(TCellPtr::null());
    let _global_side_trace_cell_addr = (&*global_side_trace_box) as *const TCellPtr as i64;

    // `regs_full` is sized to `window_size_us`, big
    // enough for every inlined frame's register window. Slots
    // [0..max_stack) are loaded from reg_state (caller-marshalled);
    // [max_stack..window_size_us) start as `iconst(0)` so the
    // callee's `GetUpval` / arith fills them. The emit loop below
    // shadows `regs` to the per-op window slice so existing
    // `regs[ins.X()]` indexing automatically shifts across inlined
    // frames without rewriting every access.
    let mut regs_full: Vec<Variable> = Vec::with_capacity(window_size_us);
    for i in 0..window_size_us {
        let v = bcx.declare_var(types::I64);
        if i < max_stack {
            let offset = (i as i32) * 8;
            let v0 = bcx
                .ins()
                .load(types::I64, MemFlagsData::new(), reg_state, offset);
            bcx.def_var(v, v0);
        } else {
            let z = bcx.ins().iconst(types::I64, 0);
            bcx.def_var(v, z);
            // Exits store only what changed since (see `sync_reg_state`),
            // so reg_state must hold the zero too: a side trace entered
            // from its parent's exit finds the parent's values here.
            bcx.ins()
                .store(MemFlagsData::new(), z, reg_state, (i as i32) * 8);
        }
        regs_full.push(v);
    }
    // Variable carrying R[A+4]'s tag byte across the
    // TForCall body emit → TForLoop tail emit boundary. TForCall's
    // batched helper returns the tag on success; tail emit reads
    // it via use_var to dispatch on Nil / Int / other instead of
    // calling the `luna_jit_stack_tag` helper. Declared
    // unconditionally — only def_var'd if the trace actually has a
    // TForCall (otherwise unused, cranelift tree-shakes).
    let tforcall_tag_var = bcx.declare_var(types::I64);
    // The tag of the value TForCall produced (R[A+5]), for the TForLoop
    // back-edge check.
    let tforcall_val_tag_var = bcx.declare_var(types::I64);
    {
        let z = bcx.ins().iconst(types::I64, 0);
        bcx.def_var(tforcall_tag_var, z);
        bcx.def_var(tforcall_val_tag_var, z);
    }

    // depth-relative `base_var` scaffold.
    //
    // The Variable is declared at trace head (here, in the entry
    // block immediately after the reg_state load prelude) and
    // initialised to `iconst(0)` as the depth-0 sentinel
    // placeholder. No op-arm reads it yet; they still index
    // `regs_full[off + slot]`.
    //
    // An unused Variable initialized via a single iconst gets DCE'd
    // by Cranelift's mid-end, so the scaffold is overhead-neutral.
    //
    // Probe: `BASE_VAR_SCAFFOLD_DECLARED` bumps exactly once at the
    // post-def_var point so the regression test
    // `base_var_scaffold.rs` can assert "scaffold ran" on
    // an arbitrary fixture trace without scraping IR text. Bump
    // happens after `def_var` so a `declare_var` panic earlier leaves
    // the counter unchanged.
    let base_var = bcx.declare_var(types::I64);
    {
        let z = bcx.ins().iconst(types::I64, 0);
        bcx.def_var(base_var, z);
        // Mirror the tforcall_tag_var declaration pattern exactly
        // (declare + iconst init + def_var, no anchor use). Cranelift
        // tree-shakes the unused Variable in optimized builds, so the
        // scaffold adds zero machine-code residue.
        BASE_VAR_SCAFFOLD_DECLARED.with(|c| c.set(c.get().wrapping_add(1)));
    }
    EntryRegs {
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
    }
}

/// Starts the string buffer of the accumulator idiom, when the trace
/// has one, and returns what its exits need to flush it.
pub(super) fn start_accum<E: Emit>(
    bcx: &mut E,
    pl: &Plan<'_>,
    h: Helpers,
    regs_full: &[Variable],
) -> Option<FlushCtx> {
    let Plan { active_accum, .. } = *pl;
    let RuntimeHelpers {
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
        ..
    } = h.rt;

    // `flush_ctx` is declared mut here so
    // the entry-block setup below can populate it with
    // `Some(FlushCtx { ... })` when an active_accum is detected.
    // The 19 `emit_store_back_and_return_*` call sites all read
    // `flush_ctx.as_ref()`; the helpers no-op when it's None.
    let mut flush_ctx: Option<FlushCtx> = None;
    // if an active_accum is in play,
    // declare buf_var, emit acquire IR, and populate flush_ctx
    // with Some(FlushCtx { ... }). All 19 existing
    // emit_store_back_and_return_* call sites then auto-flush
    // (intern → def_var(accum_slot) → release) before storing
    // back to reg_state.
    if let Some(ref ba) = active_accum {
        let buf_var = bcx.declare_var(types::I64);
        let acquire_ref = bcx.import_func(str_buf_acquire_id);
        let intern_ref = bcx.import_func(str_buf_intern_id);
        let release_ref = bcx.import_func(str_buf_release_id);
        let extend_ref = bcx.import_func(str_buf_extend_id);
        let call_inst = bcx.ins().call(acquire_ref, &[]);
        let ptr = bcx.inst_results(call_inst)[0];
        bcx.def_var(buf_var, ptr);
        // prepend the accumulator slot's current
        // bytes into the buffer. The dispatcher always fires on
        // iter 2+ (interp's TForLoop trigger fires AFTER iter 1's
        // body has run), so by the time the trace fn entry
        // executes, `R[accum_slot]` already holds the result of
        // `s` after iter 1 (= entry_s_initial + piece_1). Without
        // this prepend, the flush at exit produces only iter 2..N
        // bytes; the test workload `s = '[' .. iter1 .. ...` loses
        // the leading `[piece_1`. Net effect: buf = accum_slot's
        // entry bytes + iter 2..N piece bytes; flush intern's all
        // bytes; correct result.
        let accum_raw = bcx.use_var(regs_full[ba.accum_slot as usize]);
        let buf_ptr = bcx.use_var(buf_var);
        let _ = bcx.ins().call(extend_ref, &[buf_ptr, accum_raw]);
        flush_ctx = Some(FlushCtx {
            buf_var,
            accum_slot: ba.accum_slot,
            intern_ref,
            release_ref,
        });
    }
    flush_ctx
}

/// Creates the loop head (and the math-fold precheck block, when the
/// folds are checked once) and jumps there from the entry block.
pub(super) fn open_body_loop<E: Emit>(
    bcx: &mut E,
    pl: &Plan<'_>,
) -> (Option<Block>, Option<Block>, Option<Block>, Block) {
    let Plan {
        record,
        effective_end,
        ..
    } = *pl;
    let math_folds = &pl.math_folds;
    // Nothing in the trace can reassign `math.<fn>` unless it stores a
    // field of that name or `math` itself, or stores under a key it does
    // not know (SetTable); calls end the trace and the table helpers
    // deopt on `__newindex`. Without such a store the math folds are
    // checked once, in `precheck` before the loop head, rather than on
    // every iteration.
    let fold_check_once = !record.ops[..effective_end].iter().any(|rop| {
        let key = |k: u32| match rop.proto.consts.get(k as usize) {
            Some(luna_core::runtime::Value::Str(s)) => Some(s.as_bytes()),
            _ => None,
        };
        match rop.inst.op() {
            Op::SetTable => true,
            // the key is K[B] for both
            Op::SetField | Op::SetTabUp => match key(rop.inst.b()) {
                Some(name) => {
                    name == b"math"
                        || name == b"string"
                        || math_folds.iter().any(|f| f.fn_name.as_bytes() == name)
                }
                None => true,
            },
            _ => false,
        }
    });
    // Filled in below, once the exit bookkeeping exists.
    let precheck = (fold_check_once && !math_folds.is_empty()).then(|| bcx.create_block());
    // the read-only tests before the loop head (see `readonly`)
    let ro_precheck = record.ops[..effective_end]
        .iter()
        .any(|rop| matches!(rop.inst.op(), Op::SetField | Op::SetI | Op::SetTable))
        .then(|| bcx.create_block());

    // the step sign of a 5.3 integer loop (see `step_guard`)
    let step_precheck = pl.step_guard.is_some().then(|| bcx.create_block());

    let body_loop = bcx.create_block();
    bcx.ins().jump(
        precheck
            .or(ro_precheck)
            .or(step_precheck)
            .unwrap_or(body_loop),
        &[],
    );
    // `body_loop` is entered after the precheck block is emitted (below):
    // reading a register there first would leave it half-built while
    // another block is emitted, which the builder rejects.
    (precheck, ro_precheck, step_precheck, body_loop)
}
