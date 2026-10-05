//! Lowering a planned compute chunk to LLVM IR.

use super::emitter::ComputeEmitter;
use super::plan::{ChunkPlan, jmp_target};
use super::{declare_jit_helpers, finalize_module};
use crate::operands::{int_compare, is_compare};
use crate::storage::EnginePair;
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use luna_core::vm::isa::{Inst, Op};

/// Lower a compute-path chunk into a JIT entry.
///
/// Emit shape (with control flow):
/// ```text
/// extern "C" fn luna_jit_llvm_entry() -> i64 {
///     bb_0:                              ; entry — alloca + sequential IR
///         %regs = alloca [N x i64]       ; N = plan.num_regs
///         ; per-op IR for each reachable PC in BB 0
///         br bb_<jump-target>            ; or condbr / ret
///     bb_<pc>:                           ; one LLVM BB per ChunkPlan::bb_starts[pc]
///         ; per-op IR for each reachable PC in this BB
///         br / condbr / ret              ; terminator
///     ...
/// }
/// ```
///
/// Per-PC emit:
/// - `LoadI rA, sBx`     → store i64 sBx, regs[A]
/// - `LoadNil rA, B`     → store i64 0 for regs[A..=A+B]
/// - `Move rA, rB`       → load regs[B]; store regs[A]
/// - `Add|Sub|Mul rA,rB,rC` → load regs[B]; load regs[C]; <iop>; store
/// - `Mod rA, rB, rC`    → load, srem, sign-fixup select, store
/// - `AddI|AddK|…`       → as the register form, with the immediate or
///                          integer constant in place of regs[C]
/// - `Return0`           → ret i64 0
/// - `Return1 rA`        → load regs[A]; ret
/// - `Jmp`               → br bb_<target>
/// - `Lt|Le|Eq rA,rB,k`  → load, icmp, condbr (k flips arms; pc+1 Jmp
///                          provides the false-edge target)
/// - `LtI|GtI|EqK|…`     → the same, comparing regs[A] with the
///                          immediate or integer constant
pub(super) fn compile_compute_chunk(plan: &ChunkPlan) -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: the `Context` lives in a box whose address does not
    // change when the box moves. Everything made from `ctx_static`
    // here (module, builder, types, values) is a local declared after
    // `ctx_box`, so it drops first on every early return; on success
    // `finalize_module` moves the box into the `EnginePair` next to the
    // engine, whose field order drops the engine before the context.
    let ctx_static: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };

    let module = ctx_static.create_module("luna_jit_llvm_compute");
    let builder = ctx_static.create_builder();

    let i64_type = ctx_static.i64_type();
    let regs_ty = i64_type.array_type(plan.num_regs);
    // Parametric chunks. The fn signature widens
    // from `fn() -> i64` to `fn(i64, …, i64) -> i64` with one i64 per
    // declared positional param, then the entry BB stores each
    // function arg into the matching `regs[i]` slot so the lowerer
    // sees param 0..N-1 as live register sources.
    let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> =
        (0..plan.num_params).map(|_| i64_type.into()).collect();
    let fn_type = i64_type.fn_type(&param_types, false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);

    // Declare every `luna_jit_*` helper as an
    // external IR function. Used by Op::GetUpval / Op::Call emit
    // below; the dead-locals path skips this step.
    let helpers = declare_jit_helpers(ctx_static, &module);

    // Pre-create one LLVM BB per source BB. The PC-keyed map gives
    // O(1) lookup for branch targets.
    let n = plan.code.len();
    let mut bb_of_pc: Vec<Option<BasicBlock<'static>>> = vec![None; n];
    for (pc, start) in plan.bb_starts.iter().enumerate() {
        if *start && plan.reachable[pc] {
            bb_of_pc[pc] = Some(ctx_static.append_basic_block(function, &format!("bb_{pc}")));
        }
    }
    let entry_bb = bb_of_pc[0]?;
    builder.position_at_end(entry_bb);

    // Allocate the chunk's register file in the entry BB. All other
    // BBs read/write via the same alloca pointer; LLVM's mem2reg /
    // SROA promote the scalar slots out of memory.
    let regs = builder.build_alloca(regs_ty, "regs").ok()?;

    let mut emitter = ComputeEmitter {
        ctx: ctx_static,
        builder: &builder,
        function,
        i64_type,
        regs_ty,
        regs,
        helpers: &helpers,
    };

    // Populate `regs[0..num_params]` from the fn
    // arg list. After this prologue the lowerer sees param 0..N-1 as
    // ordinary live register sources, identical to a chunk that
    // bound them with `LoadI` / `Move`.
    for i in 0..plan.num_params {
        let slot = emitter.reg_slot_ptr(i, "param_slot")?;
        let arg = function.get_nth_param(i)?.into_int_value();
        builder.build_store(slot, arg).ok()?;
    }

    // Self-recursive calls are direct calls to this code, right only
    // while the upvalue they go through still holds the running closure.
    if let Some(idx) = plan.self_upval_idx {
        let check = helpers.get("luna_jit_self_upval_check").copied()?;
        let idx_arg = i64_type.const_int(u64::from(idx), false);
        emitter.return_unless(check, &[idx_arg.into()], "self")?;
    }

    // Walk PCs; switch BB on bb_starts boundaries; terminators
    // (Return*/Jmp/Lt|Le|Eq) handled here; non-CF ops delegated to
    // `emitter.emit_op`.
    let mut bb_terminated = false;
    let mut current_bb = Some(entry_bb);
    let mut pc = 0usize;
    while pc < n {
        // Skip unreachable ops — they have no BB and would not be
        // valid emit targets.
        if !plan.reachable[pc] {
            pc += 1;
            continue;
        }

        // Entering a new BB? Either we just emitted a terminator (in
        // which case we MUST switch) or the prev BB fell through to
        // a BB-start (insert an unconditional br).
        if plan.bb_starts[pc] && current_bb != bb_of_pc[pc] {
            let next_bb = bb_of_pc[pc]?;
            if !bb_terminated {
                builder.build_unconditional_branch(next_bb).ok()?;
            }
            builder.position_at_end(next_bb);
            current_bb = Some(next_bb);
            bb_terminated = false;
        }

        // Consumed Jmp (folded into a preceding Lt|Le|Eq condbr) —
        // skip emit entirely. The condbr already wrote the
        // terminator for this BB.
        if plan.consumed_jmp[pc] {
            pc += 1;
            continue;
        }

        let ins = plan.code[pc];
        match ins.op() {
            Op::Return0 => {
                let zero = i64_type.const_zero();
                builder.build_return(Some(&zero)).ok()?;
                bb_terminated = true;
            }
            Op::Return1 => {
                let v = emitter.load_reg(ins.a(), "ret_val")?;
                builder.build_return(Some(&v)).ok()?;
                bb_terminated = true;
            }
            Op::Jmp => {
                let tgt = jmp_target(pc, ins);
                let tgt_bb = bb_of_pc.get(tgt).copied().flatten()?;
                builder.build_unconditional_branch(tgt_bb).ok()?;
                bb_terminated = true;
            }
            op if is_compare(op) => {
                emit_compare_branch(&emitter, plan, &bb_of_pc, pc, ins)?;
                bb_terminated = true;
            }
            Op::GetUpval => {
                emitter.emit_get_upval(ins, plan.is_upval_value_read[pc])?;
            }
            Op::Call => {
                // Only the self-recursive shape lands here
                // (whitelist + tracker gated). Emit a direct
                // `build_call` against the current entry function and
                // store the i64 result into regs[A].
                debug_assert!(
                    plan.self_call_pcs[pc],
                    "scanner accepts only self-recursive Op::Call PCs"
                );
                emitter.emit_self_call(ins)?;
            }
            Op::TailCall => {
                // Self-recursive tail call (tracker gated).
                // Semantics: call function(R[A+1..A+nargs]) and return
                // its result directly to our caller. Emits as
                // build_call + ret, equivalent to Op::Call + Return1.
                debug_assert!(
                    plan.tail_call_pcs[pc],
                    "scanner accepts only self-recursive Op::TailCall PCs"
                );
                emitter.emit_self_tail_call(ins)?;
                bb_terminated = true;
            }
            _ => {
                emitter.emit_op(ins, plan.consts)?;
            }
        }
        pc += 1;
    }

    // Sanity: a well-formed chunk's last reachable BB ends with a
    // terminator (every parser-emitted chunk ends with `Return*`).
    // If we somehow exited the loop with an unterminated BB, the
    // resulting LLVM IR would be malformed — bail rather than emit it.
    if !bb_terminated {
        return None;
    }

    finalize_module(ctx_box, module, Some(&helpers))
}

/// Lower a comparison and the `Jmp` it consumes into one `condbr`.
fn emit_compare_branch(
    emitter: &ComputeEmitter<'static, '_>,
    plan: &ChunkPlan,
    bb_of_pc: &[Option<BasicBlock<'static>>],
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let builder = emitter.builder;
    // Lua predicate semantics:
    //   if ((R[A] <op> R[B]) ~= k) then pc++
    // i.e. SKIP the next Jmp when the comparison's truth
    // value differs from k. Mapped to a single LLVM
    // condbr by picking the (then/else) branches per k:
    //   k = true  → then = jmp_bb, else = fall_bb
    //   k = false → then = fall_bb, else = jmp_bb
    // because:
    //   cmp == k  ↔ DON'T skip ↔ take the Jmp
    //   cmp != k  ↔ SKIP       ↔ take the fall-through
    // The immediate and constant forms compare R[A] with
    // the operand folded into the instruction.
    let (pred, rhs) = int_compare(ins, plan.consts)?;
    let lhs = emitter.load_reg(ins.a(), "cmp_lhs")?;
    let rhs = emitter.operand_value(rhs, "cmp_rhs")?;
    let cmp = builder.build_int_compare(pred, lhs, rhs, "cmp_res").ok()?;
    let jmp_ins = plan.code.get(pc + 1)?;
    let jmp_pc = pc + 1;
    let jmp_target_pc = jmp_target(jmp_pc, *jmp_ins);
    let fall_pc = pc + 2;
    let fall_bb = bb_of_pc.get(fall_pc).copied().flatten()?;
    let jmp_bb = bb_of_pc.get(jmp_target_pc).copied().flatten()?;
    let (then_bb, else_bb) = if ins.k() {
        (jmp_bb, fall_bb)
    } else {
        (fall_bb, jmp_bb)
    };
    builder
        .build_conditional_branch(cmp, then_bb, else_bb)
        .ok()?;
    Some(())
}
