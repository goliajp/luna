use super::*;

/// What the emit pass reads.
pub(super) struct EmitIn<'a> {
    pub(super) c: ChunkIn<'a>,
    pub(super) scan: &'a ChunkScan,
    pub(super) cfg: &'a ChunkCfg,
    pub(super) reg_kinds: &'a [RegKind],
    pub(super) ret_kind: RegKind,
    pub(super) presize_for_newtable: &'a std::collections::HashMap<usize, i64>,
    pub(super) bb_entry_kinds: &'a [Vec<RegKind>],
    pub(super) arg_float_mask: u8,
    pub(super) arg_table_mask: u8,
    pub(super) any_self_call: bool,
}

/// What every op's emit reads.
#[derive(Clone, Copy)]
pub(super) struct EmitFacts<'a> {
    pub(super) c: ChunkIn<'a>,
    pub(super) scan: &'a ChunkScan,
    pub(super) reg_kinds: &'a [RegKind],
    pub(super) ret_kind: RegKind,
    pub(super) presize_for_newtable: &'a std::collections::HashMap<usize, i64>,
    pub(super) regs: &'a [Variable],
    pub(super) pc_to_block: &'a [Option<Block>],
    pub(super) fn_id: FuncId,
    /// set when the chunk calls itself: see [`SelfCalls`]
    pub(super) self_calls: Option<SelfCalls>,
}

/// A self-recursive chunk's entry puts the stack limit in the pinned
/// register, where its body's every call reads it at no cost to the
/// calls themselves. The body takes more parameters, after its own: for
/// 5.1 and 5.2 (`float_only`; only 5.1 has a depth limit) the calls left;
/// and where going on after a failed self call could be seen (a table
/// write) or might not end (a backward jump) the address of the entry's
/// context (`luna_jit_enter_ctx`), whose failure flag
/// `luna_jit_self_call_slow` sets, so that every caller returns at once.
/// Elsewhere the callers finish with dummy results that the dispatcher
/// drops. Before the first self call of each block (the stack pointer
/// does not move within the body, so once is enough) the body checks the
/// stack pointer against the limit and whether the callee would have a
/// call left; failing either, `luna_jit_self_call_slow` makes the call, or,
/// in a chunk with no context, the whole of this call again.
#[derive(Clone, Copy)]
pub(super) struct SelfCalls {
    /// the calls left, when counted
    pub(super) left: Option<Value>,
    /// the context, when the failure flag is checked
    pub(super) ctx: Option<Value>,
    /// `luna_jit_helpers::self_call_desc` of the calls
    pub(super) desc: i64,
    /// the body's own raw parameters, for running this whole call in the
    /// interpreter instead (a chunk without a context: nothing it did so
    /// far can be seen, so doing it again is not either)
    pub(super) own: [Option<Value>; 4],
}

/// Which of the [`SelfCalls`] parameters a chunk's body takes.
#[derive(Clone, Copy)]
pub(super) struct SelfCallParams {
    pub(super) count: bool,
    pub(super) ctx: bool,
}

impl SelfCallParams {
    pub(super) fn of(c: ChunkIn<'_>) -> Self {
        SelfCallParams {
            count: c.float_only,
            ctx: may_show_dummy_results(c),
        }
    }

    pub(super) fn len(self) -> usize {
        usize::from(self.count) + usize::from(self.ctx)
    }
}

/// What every op's emit updates.
pub(super) struct EmitState {
    pub(super) current_kinds: Vec<RegKind>,
    pub(super) current_is_nil: Vec<bool>,
    pub(super) terminated: bool,
    /// whether a self call in this block must go through
    /// `luna_jit_self_call_slow`, once the block's first one has checked
    pub(super) self_call_slow: Option<Value>,
}

/// Emits the chunk body, then its checked entry when it needs one, and
/// returns the entry.
pub(super) fn emit_chunk<M: Module>(module: &mut M, e: EmitIn<'_>) -> Option<FuncId> {
    let EmitIn {
        c,
        scan,
        cfg,
        reg_kinds,
        ret_kind,
        presize_for_newtable,
        bb_entry_kinds,
        arg_float_mask,
        arg_table_mask,
        any_self_call,
    } = e;
    let ChunkIn {
        proto,
        code,
        n,
        num_params,
        ..
    } = c;
    let ChunkScan {
        bb_starts,
        folded_math,
        ..
    } = scan;
    let pc_to_bb = &cfg.pc_to_bb;
    let mut sig = module.make_signature();
    for _ in 0..num_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    let extra = SelfCallParams::of(c);
    if any_self_call {
        for _ in 0..extra.len() {
            sig.params.push(AbiParam::new(types::I64));
        }
    }
    sig.returns.push(AbiParam::new(types::I64));
    let fn_id = module
        .declare_function("luna_jit_chunk", Linkage::Local, &sig)
        .ok()?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());

    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

    // Create one cranelift Block per Lua basic block, indexed by the
    // BB's leading PC. The entry block also gets the Variable-declaration
    // prelude so the chunk has well-defined register values from PC 0.
    let mut pc_to_block: Vec<Option<Block>> = vec![None; n];
    for pc_i in 0..n {
        if bb_starts[pc_i] {
            pc_to_block[pc_i] = Some(bcx.create_block());
        }
    }
    let entry = pc_to_block[0].expect("entry block exists");
    // Append the entry block's function-param block params before
    // switching in, so the params arrive as block args we can read
    // straight into the register Variables.
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);

    // Variables = Lua registers, declared once on the entry block so
    // every BB downstream can `use_var` / `def_var` them. Each register
    // gets a Cranelift type chosen from `reg_kinds[i]` (Int → I64,
    // Float → F64). Unset registers default to I64 — they're unreachable
    // in well-formed Lua but we still need a valid SSA shape.
    // two more registers: the constant operands' scratch (`split_const_operands`)
    let max_stack = (proto.max_stack as usize).max(num_params) + 2;
    let regs = declare_regs(&mut bcx, c, reg_kinds, entry, max_stack);
    // emit-side per-PC kind tracker. Initialized from
    // the per-arg masks (Float bit → Float, Table bit → Table, else
    // Int) and updated forward at every writer op below. Used by
    // `SetList` to tag-store each element correctly and by
    // arith/cmp ops in lieu of the global `reg_kinds` slot when the
    // global slot has been "joint-pinned" by Int + Table re-use.
    let mut current_kinds: Vec<RegKind> = vec![RegKind::Unset; max_stack];
    for i in 0..num_params {
        current_kinds[i] = if (arg_float_mask >> i) & 1 == 1 {
            RegKind::Float
        } else if (arg_table_mask >> i) & 1 == 1 {
            RegKind::Table
        } else {
            RegKind::Int
        };
    }
    // parallel to `current_kinds`: tracks "the value
    // last written here is a Nil sentinel (raw bits = 0)". Set by
    // `Op::LoadNil` emit; cleared by any other writer touching the
    // same register. Reset to all-false at every BB switch (the
    // narrow LoadNil → SetList window we lower lives entirely in
    // one BB; broader BB-level Nil dataflow is left for later if
    // a wider pattern needs it). `SetList` emit reads this to pick
    // `RAW_TAG_NIL` over the default Int tag, so a chunk like
    // `binary_trees`'s `{nil, nil}` leaf stores actual Nil values
    // instead of misinterpreting the 0 bits as `Int(0)`.
    let current_is_nil: Vec<bool> = vec![false; max_stack];
    let self_calls = any_self_call.then(|| {
        let params = bcx.block_params(entry).to_vec();
        let mut rest = params[num_params..].iter().copied();
        SelfCalls {
            left: if extra.count { rest.next() } else { None },
            ctx: if extra.ctx { rest.next() } else { None },
            desc: self_call_desc((arg_float_mask, arg_table_mask), num_params, scan, ret_kind),
            own: std::array::from_fn(|i| params[..num_params].get(i).copied()),
        }
    });

    let f = EmitFacts {
        c,
        scan,
        reg_kinds,
        ret_kind,
        presize_for_newtable,
        regs: &regs,
        pc_to_block: &pc_to_block,
        fn_id,
        self_calls,
    };
    let mut st = EmitState {
        current_kinds,
        current_is_nil,
        terminated: false,
        self_call_slow: None,
    };
    let mut current_block = entry;
    let mut pc = 0;
    while pc < n {
        // Entering a new BB: if the previous BB fell through without a
        // terminator, append an explicit jump so cranelift's verifier
        // doesn't choke.
        if pc != 0 && bb_starts[pc] {
            let next_blk = pc_to_block[pc].expect("BB present");
            if !st.terminated {
                bcx.ins().jump(next_blk, &[]);
            }
            bcx.switch_to_block(next_blk);
            current_block = next_blk;
            st.terminated = false;
            st.self_call_slow = None;
            // reset emit-side `current_kinds` to
            // the per-BB dataflow result so an alternate-path
            // writer's kind doesn't leak forward. The linear writer
            // updates below continue to refine `current_kinds` as
            // emit progresses through the new BB.
            let new_bb_idx = pc_to_bb[pc];
            st.current_kinds = bb_entry_kinds[new_bb_idx].clone();
            // Nil writes don't cross BB joins in the
            // patterns we lower; reset rather than fold them into
            // a separate per-BB dataflow.
            for slot in st.current_is_nil.iter_mut() {
                *slot = false;
            }
        }
        let _ = current_block; // tracked only for parity assertions in tests.
        let ins = code[pc];
        match ins.op() {
            Op::LoadI
            | Op::LoadF
            | Op::LoadK
            | Op::LoadNil
            | Op::Move
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Return1
            | Op::Return0
            | Op::Jmp => emit_basic::emit_basic(&mut bcx, &mut st, f, pc, ins)?,
            Op::GetTabUp => pc = emit_calls::emit_math_fold(module, &mut bcx, &mut st, f, pc)?,
            Op::GetField if folded_math[pc] => {
                unreachable!("GetTabUp emit advances pc past the rest of the fold");
            }
            Op::GetUpval => emit_calls::emit_get_upval(module, &mut bcx, &mut st, f, pc, ins)?,
            Op::Call => emit_calls::emit_self_call(module, &mut bcx, &mut st, f, pc, ins)?,
            Op::ForPrep => emit_for::emit_for_prep(&mut bcx, &mut st, f, pc, ins),
            Op::ForLoop => emit_for::emit_for_loop(&mut bcx, &mut st, f, pc, ins),
            Op::Lt | Op::Le | Op::Eq => pc = emit_basic::emit_cmp(&mut bcx, &mut st, f, pc, ins),
            Op::NewTable => emit_table_set::emit_new_table(module, &mut bcx, &mut st, f, pc, ins)?,
            Op::SetTable => emit_table_set::emit_set_table(module, &mut bcx, f, ins)?,
            Op::SetList => emit_table_set::emit_set_list(module, &mut bcx, &mut st, f, pc, ins)?,
            Op::GetI => emit_table_get::emit_get_i(module, &mut bcx, &mut st, f, ins)?,
            Op::GetTable => emit_table_get::emit_get_table(module, &mut bcx, &mut st, f, ins)?,
            Op::Len => emit_table_get::emit_len(module, &mut bcx, &mut st, f, ins)?,
            _ => return None,
        }
        pc += 1;
    }
    if !st.terminated {
        let zero = bcx.ins().iconst(types::I64, 0);
        bcx.ins().return_(&[zero]);
    }
    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());

    module.define_function(fn_id, &mut ctx).ok()?;
    chunk_share::note(module, &ctx, fn_id);
    module.clear_context(&mut ctx);

    let extra = any_self_call.then_some(extra);
    define_entry(module, &mut ctx, fn_id, scan, extra, num_params)
}

fn declare_regs(
    bcx: &mut FunctionBuilder<'_>,
    c: ChunkIn<'_>,
    reg_kinds: &[RegKind],
    entry: Block,
    max_stack: usize,
) -> Vec<Variable> {
    let ChunkIn { num_params, .. } = c;
    let mut regs: Vec<Variable> = Vec::with_capacity(max_stack);
    let entry_block_params: Vec<_> = bcx.block_params(entry).to_vec();
    for i in 0..max_stack {
        let cl_ty = match reg_kinds.get(i).copied().unwrap_or(RegKind::Unset) {
            RegKind::Float => types::F64,
            // Table is a `Gc<Table>` pointer pun — I64-shaped at the
            // Cranelift level, distinct in the kind lattice.
            RegKind::Int | RegKind::Unset | RegKind::Table => types::I64,
        };
        let v = bcx.declare_var(cl_ty);
        // Lua call ABI: arg `i` lands in register `i`. The cranelift
        // entry signature is i64; for a Float param we bitcast the i64
        // bit-pattern back to f64 here. Params past num_params are
        // zero-initialised in their target type.
        let init = if i < num_params {
            let raw = entry_block_params[i];
            if cl_ty == types::F64 {
                bcx.ins().bitcast(types::F64, MemFlagsData::new(), raw)
            } else {
                raw
            }
        } else if cl_ty == types::F64 {
            bcx.ins().f64const(0.0)
        } else {
            bcx.ins().iconst(types::I64, 0)
        };
        bcx.def_var(v, init);
        regs.push(v);
    }
    regs
}

fn define_entry<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    fn_id: FuncId,
    scan: &ChunkScan,
    self_calls: Option<SelfCallParams>,
    num_params: usize,
) -> Option<FuncId> {
    let any_self_call = self_calls.is_some();
    let ChunkScan {
        self_upval_idx,
        math_folds,
        ..
    } = scan;
    // The body's self-recursive calls go straight to its own code, which
    // is the Lua call only while the upvalue they load holds the running
    // closure, and its math folds replace `math.<fn>(...)` by inline code,
    // which is the Lua call only while the field holds the library
    // function. The compiled code is shared by every closure of the proto
    // (and by protos with the same code), so both are checked on each
    // entry from the interpreter. Recursive calls enter the body directly:
    // nothing the body runs can reassign the upvalue or, with no table
    // stores (checked above), a field.
    let mut math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)> = Vec::new();
    for fold in math_folds {
        if !math_fns.iter().any(|&(_, n)| n.ptr_eq(fold.name_key)) {
            math_fns.push((fold.math_key, fold.name_key));
        }
    }
    let checks = EntryChecks {
        self_upval: self_upval_idx.filter(|_| any_self_call),
        math_fns,
        self_calls,
    };
    let entry_id = if any_self_call || !checks.math_fns.is_empty() {
        define_checked_entry(module, ctx, fn_id, &checks, num_params)?
    } else {
        fn_id
    };
    Some(entry_id)
}

/// `luna_jit_helpers::self_call_desc` of a chunk's self calls.
fn self_call_desc(masks: (u8, u8), num_params: usize, scan: &ChunkScan, ret_kind: RegKind) -> i64 {
    use luna_jit_helpers::*;
    let ret = match (scan.sees_return1, ret_kind) {
        (false, _) => SELF_CALL_RET_NONE,
        (true, RegKind::Float) => SELF_CALL_RET_FLOAT,
        (true, RegKind::Table) => SELF_CALL_RET_TABLE,
        (true, _) => SELF_CALL_RET_INT,
    };
    luna_jit_helpers::self_call_desc(num_params as u32, masks.0, masks.1, ret)
}

/// Whether computing on after a failed self call could be seen or might
/// not end: a table write, a new table, or a backward jump.
fn may_show_dummy_results(c: ChunkIn<'_>) -> bool {
    c.code.iter().enumerate().any(|(pc, &ins)| match ins.op() {
        Op::SetTable | Op::SetList | Op::NewTable => true,
        Op::Jmp => jmp_target(pc, ins) <= pc,
        _ => false,
    })
}
