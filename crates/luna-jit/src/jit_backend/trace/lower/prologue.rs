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
pub(super) fn emit_entry<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
    pl: &Plan<'_>,
) -> EntryRegs {
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
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        bcx.func.import_signature(sig)
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

/// Declares the virtual registers of each sinkable table site, demoting
/// the sites that cannot be sunk.
pub(super) fn alloc_sunk_sites(
    bcx: &mut FunctionBuilder<'_>,
    pl: &Plan<'_>,
    escape: &mut EscapeAnalysis,
) -> (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32) {
    let Plan {
        record,
        end_idx_opt,
        ..
    } = *pl;
    // allocate virtual `Variable`s for each Sinkable
    // site that meets the sunk-emit criteria. Sites that don't
    // meet the criteria are demoted to Escaped right here so the
    // body emit's site-state check naturally falls through to the
    // existing heap-alloc helper path. Criteria:
    //   - `inline_depth == 0` (trace head's frame only — inline
    //     sinking requires extra plumbing for
    //     the materialize helper to address inlined windows)
    //   - `array_cap` in `1..=MAX_SUNK_CAP` (cap = 0 means the
    //     site didn't decode an array part; cap > MAX is a
    //     Cranelift Variable budget guard)
    //   - the site's slot is NOT the trace-terminator `Op::Return1`
    //     R[A] — sinking that case needs the materialize helper
    //     to repack the array into a heap `Gc<Table>` on the way
    //     out
    //   - the trace's body has NO cmp ops (`Lt`/`Le`/`Eq`/`EqK`) —
    //     a cmp emits a side-exit and the interp resume needs the
    //     heap table; the sweep escapes all live bindings on
    //     a cmp, but we ALSO need to bail on body cmps that fire
    //     AFTER the site dies (no live binding to escape, but the
    //     trace still has a back-edge candidate).
    //
    // Note: looping traces (`opts.internal_loop = true`) that have
    // any cmp in body are already excluded by the sweep escape
    // rule. ForLoop terminators escape via the terminator rule
    // (TraceEnd::ForLoop → all live). So we don't need an explicit
    // `internal_loop` check here.
    const MAX_SUNK_CAP: u32 = 8;
    let return_a_for_sunk_check: Option<u32> = match end_idx_opt {
        Some((idx, TraceEnd::Return)) if idx < record.ops.len() => {
            let term = &record.ops[idx];
            if matches!(term.inst.op(), Op::Return1) && term.inline_depth == 0 {
                Some(term.inst.a())
            } else {
                None
            }
        }
        _ => None,
    };
    // There is no inline-cmp gate: inline cmp
    // side-exits (per_exit_inline arm) call
    // `emit_materialize_live_sunk` to reconstruct live sunk sites
    // before the frame-mat helper pushes inline frames, so a
    // depth>0 cmp doesn't demote sites.
    let mut virt_vars: Vec<Option<Vec<Variable>>> = vec![None; escape.sites.len()];
    let mut virt_kinds: Vec<Option<Vec<RegKind>>> = vec![None; escape.sites.len()];
    let mut sunk_alloc_seen: u32 = 0;
    for (idx, site) in escape.sites.iter_mut().enumerate() {
        if site.state != EscapeState::Sinkable {
            continue;
        }
        // depth>0 sites are sunk-eligible. Materialise
        // (`emit_materialize_live_sunk`) handles BOTH depth=0 and
        // depth>0 sites at depth=0 cmp arm AND inline cmp
        // (per_exit_inline) arm, since inline cmp side-exits
        // reconstruct live sunk sites. `return_a` check only matters for depth=0
        // (TraceEnd::Return applies at the trace-head frame).
        // total virt slot count = array_cap + hash_keys.
        // - array-only site:    cap = array_cap,           hash = 0
        // - hash-only site:     cap = 0,                   hash = hash_keys.len()
        // - mixed array+hash:   cap = array_cap > 0,       hash > 0
        // - empty (no ops):     cap = 0,                   hash = 0 → demoted below
        let array_cap = site.array_cap as usize;
        let n_hash = site.hash_keys.len();
        let total_slots = array_cap + n_hash;
        if total_slots == 0
            || array_cap > MAX_SUNK_CAP as usize
            || (site.inline_depth == 0 && return_a_for_sunk_check == Some(site.a))
        {
            site.state = EscapeState::Escaped;
            continue;
        }
        // hash slot materialise is plumbed into
        // emit_materialize_live_sunk (extended helper signature
        // carries hash_keys + hash_raws + hash_kinds buffers), so no
        // has_any_cmp gate is needed. Hash sites survive cmp side-exits via
        // table.set(Value::Str(key), ...) at materialise time.
        let mut vars = Vec::with_capacity(total_slots);
        for _ in 0..total_slots {
            let v = bcx.declare_var(types::I64);
            let z = bcx.ins().iconst(types::I64, 0);
            bcx.def_var(v, z);
            vars.push(v);
        }
        virt_vars[idx] = Some(vars);
        virt_kinds[idx] = Some(vec![RegKind::Unset; total_slots]);
        sunk_alloc_seen += 1;
    }
    (virt_vars, virt_kinds, sunk_alloc_seen)
}

/// Starts the string buffer of the accumulator idiom, when the trace
/// has one, and returns what its exits need to flush it.
pub(super) fn start_accum<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
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
        let acquire_ref = module.declare_func_in_func(str_buf_acquire_id, bcx.func);
        let intern_ref = module.declare_func_in_func(str_buf_intern_id, bcx.func);
        let release_ref = module.declare_func_in_func(str_buf_release_id, bcx.func);
        let extend_ref = module.declare_func_in_func(str_buf_extend_id, bcx.func);
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
pub(super) fn open_body_loop(
    bcx: &mut FunctionBuilder<'_>,
    pl: &Plan<'_>,
) -> (Option<Block>, Block) {
    let Plan {
        record,
        head_proto,
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
        let key = |k: u32| match head_proto.consts.get(k as usize) {
            Some(luna_core::runtime::Value::Str(s)) => Some(s.as_bytes()),
            _ => None,
        };
        match rop.inst.op() {
            Op::SetTable => true,
            // the key is K[B] for both
            Op::SetField | Op::SetTabUp => match key(rop.inst.b()) {
                Some(name) => {
                    name == b"math" || math_folds.iter().any(|f| f.fn_name.as_bytes() == name)
                }
                None => true,
            },
            _ => false,
        }
    });
    // Filled in below, once the exit bookkeeping exists.
    let precheck = (fold_check_once && !math_folds.is_empty()).then(|| bcx.create_block());

    let body_loop = bcx.create_block();
    bcx.ins().jump(precheck.unwrap_or(body_loop), &[]);
    // `body_loop` is entered after the precheck block is emitted (below):
    // reading a register there first would leave it half-built while
    // another block is emitted, which the builder rejects.
    (precheck, body_loop)
}

/// The emit-pass state at the loop head.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_body<'f, 'm, M: Module>(
    module: &'m mut M,
    bcx: FunctionBuilder<'f>,
    pl: &Plan<'_>,
    h: Helpers,
    head: EntryRegs,
    escape: EscapeAnalysis,
    defined_aot_data: std::collections::HashSet<DataId>,
    sunk: (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32),
    flush_ctx: Option<FlushCtx>,
    blocks: (Option<Block>, Block),
) -> Lower<'f, 'm, M> {
    let EntryRegs {
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
    } = head;
    let (virt_vars, virt_kinds, sunk_alloc_seen) = sunk;
    let (precheck, body_loop) = blocks;
    let Plan {
        record,
        max_stack,
        window_size_us,
        ..
    } = *pl;
    let head_live = &pl.head_live;
    // What reg_state holds for each register at the loop head: on entry
    // the values the prelude loaded (caller window) or the zeroes the
    // dispatcher filled it with (inline frames); on the back-edge what
    // `sync_reg_state` wrote before the jump. Read at the loop head below.
    let stored: Vec<Option<Value>> = Vec::new();

    // Per-reg current kind: the recorded entry tag for a head-frame
    // register the trace reads before writing (the dispatcher checks
    // it), held on the stack for the other head-frame registers, and
    // Unset past the head frame (the dispatcher zero-initialises those
    // reg_state slots and trace IR fills them via writers). Sized to
    // `window_size_us` (mirrors `regs_full`).
    let current_kinds: Vec<RegKind> = (0..window_size_us)
        .map(|i| match head_live.get(i) {
            Some(true) => record
                .entry_tags
                .get(i)
                .and_then(|&t| RegKind::from_entry_tag(t))
                .unwrap_or(RegKind::Unset),
            Some(false) => RegKind::StackHeld,
            None => RegKind::Unset,
        })
        .collect();
    // The kinds the body is lowered for. A back-edge may run it again
    // only when the caller window holds these same kinds (see
    // `loop_kinds_match`); otherwise
    // the next pass would read (and hand to an exit) a register with
    // bits of one kind as another, e.g. an Int as a Float.
    let head_kinds: Vec<RegKind> = current_kinds[..max_stack].to_vec();
    let dispatchable: bool = true;
    // the first emit-pass site that flips
    // dispatchable to false wins this label; CompiledTrace
    // exposes it via `dispatch_off_reason` for probe diagnostics.
    let dispatch_off_reason: Option<&'static str> = None;
    // per-side-exit RegKind snapshot. Pushed at each
    // true side-exit emit site (Lt/Le/Eq + Jmp) so later writers
    // (e.g. `Op::GetUpval` whose result we infer as `Closure`) don't
    // pollute the side-exit's restore with a tag the slot hasn't
    // actually become at that exit. The clean-tail and call-truncation
    // paths reuse the final `current_kinds` via `ct.exit_tags`.
    // 3rd element is the per-entry `Box<Cell<*const
    // u8>>` whose heap address is baked into the corresponding
    // emit_store_back_and_return_pc callsite. Allocated at each push
    // site BEFORE the helper call so the IR's `iconst`-baked address
    // exists. Transported through into `tags_side_trace_ptrs` at the
    // end of emit (the cell never moves).
    let per_exit_kinds: Vec<(u32, Vec<RegKind>, Box<TCellPtr>)> = Vec::new();
    // per inline cmp@d>0 side-exit. Each entry
    // is built at the cmp emit site and includes the side-exit PC,
    // a window-sized exit-tag snapshot, and the frame-mat chain. The
    // IR encodes `(site_idx + 1)` in the upper 32 bits of the
    // returned i64 so the dispatcher can pick the right entry
    // without colliding on shared cont_pc values (fib's cmp@d=0
    // through cmp@d=4 all side-exit to the same PC).
    // 5th element is the per-site `Box<Cell<*const
    // u8>>` whose heap address is baked into the IR's
    // `emit_store_back_and_return_site` gate. Allocated at each push
    // site BEFORE the helper call (address is
    // stable across `Vec → Rc<[]>` moves because Box transfers
    // ownership without moving the heap cell).
    let per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )> = Vec::new();
    // Live call stack mirror — push on self-recursive `Op::Call`,
    // pop on `Op::Return0/1` at depth>0. Each frame's `base_offset`
    // and `pc` (= caller's Call.pc + 1) are stamped at push time;
    // when snapshotting at a cmp@d>0 site, the innermost frame's
    // `pc` is overwritten with the actual side-exit PC so the helper
    // pushes the right resume point without needing a dispatcher
    // post-hoc fix-up.
    let call_chain: Vec<FrameMaterializeInfo> = Vec::new();

    // --- emit body
    //
    // Cranelift's `FunctionBuilder` tracks the "current" block
    // internally; every `bcx.ins()` emits into whichever block was
    // last `switch_to_block`'d. A cmp's `brif` forks the current
    // block to a `continue_blk` and a `side_exit_blk`; after
    // emitting the side-exit and switching back to `continue_blk`,
    // subsequent ops land there. By the end of the loop the
    // "current" block is whatever the last cmp's continue branch
    // pointed at (or the entry block if no cmps fired).
    //
    // Only the *normal* range (`record.ops[..effective_end]`) is
    // emitted. If `Op::Call` truncates the trace, the tail emits
    // a side-exit at the Call's PC instead of the head_pc close.
    // Memoize GetUpval(idx) per dispatch.
    // For self-recursive traces (fib, factorial, etc.), the trace head
    // is entered with one closure and `JIT_CL` stays pinned to it for
    // the entire dispatch; all inlined-depth GetUpval(idx) calls return
    // the same value. Hoist the helper call to the first occurrence and
    // reuse the cached SSA value at later sites (in cranelift-dominated
    // blocks). For fib's 3-deep inline trace, this cuts 4 helper calls
    // to 1 per dispatch (~60 cycles saved × 163k dispatches ≈ 3-5 ms,
    // ~10-15% win).
    //
    // SAFETY of memoization:
    // - Cache invalidation: none required within a single trace
    //   dispatch — `JIT_CL` is pinned at entry and unchanged through
    //   the entire trace body. The first GetUpval(idx) call materializes
    //   the value; subsequent reads of the same idx are exact duplicates.
    // - Cross-block validity: cached values are stored in a Variable
    //   (via def_var / use_var); cranelift's FunctionBuilder inserts
    //   phis as needed for cross-block reads.
    // - Side-exit safety: the first occurrence may be in a block reached
    //   only on the recursive path (e.g. block2 in fib). If a side-exit
    //   fires BEFORE that block (e.g. head-fail base case in block3),
    //   the cache is never populated and reuse never happens — correct.
    let upval_cache: std::collections::HashMap<u32, Variable> = std::collections::HashMap::new();
    // the entry closure, fetched at the first inlined call; the trace
    // is linear, so that fetch dominates every later call
    let head_closure_var: Option<Variable> = None;
    // No iconst memoization: the arm64 backend folds
    // `iconst+isub`/`iconst+icmp` into immediate-form instructions
    // at codegen, so it would add little.
    // incremented at each cmp side-exit emit point that
    // materialises ≥1 live Sinkable site. Telemetry only; the
    // dispatcher's runtime materialise calls are not counted here
    // (this is a per-trace static count of emit sites that emit
    // the helper call).
    let materialize_emit_count: u32 = 0;
    let closure_seen: u32 = 0;
    // Integer constants the registers hold at this point of the trace
    // (from LoadI / LoadK earlier in the same pass), so a `//`, `%` or shift
    // by a constant needs no runtime guard.
    let known_int: Vec<Option<i64>> = vec![None; window_size_us];
    Lower {
        module,
        bcx,
        h,
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
        precheck,
        body_loop,
        head_kinds,
        defined_aot_data,
        escape,
        flush_ctx,
        virt_vars,
        virt_kinds,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        stored,
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        call_chain,
        upval_cache,
        head_closure_var,
        known_int,
    }
}
