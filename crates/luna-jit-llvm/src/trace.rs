//! v2.1 Phase 1K.G — LLVM trace JIT lowerer (MVP).
//!
//! ## Design: alloca-based register file (Risk 2 resolution)
//!
//! The Cranelift trace lowerer uses `FunctionBuilder::Variable` (phi-based
//! SSA with automatic phi insertion at back-edges). LLVM has no equivalent.
//!
//! Resolution: use the same `alloca [window_size x i64]` approach as the
//! chunk JIT. The alloca pointer is shared across the loop back-edge — no
//! phi nodes needed. `mem2reg` can promote the alloca to SSA at O1+; at
//! `OptimizationLevel::None` (current) the loads/stores go through memory.
//!
//! ## Trace function shape
//!
//! ```text
//! TraceFn: extern "C" fn(reg_state: *mut i64) -> i64
//!
//!   entry_bb:
//!     %regs = alloca [window_size x i64]
//!     ; load each reg_state[i] into regs[i]
//!     br body_loop_bb
//!
//!   body_loop_bb (+ continuation blocks):
//!     ; per-recorded-op IR (LoadI / Move / Add / Sub / Mul / Mod)
//!     ; for Lt|Le|Eq + Jmp pairs:
//!     ;   %cmp = icmp <pred> regs[A], regs[B]
//!     ;   build_conditional_branch(%cmp, continue_bb, side_exit_bb_N)
//!     br clean_tail_bb        ; at trace end (loop back to head_pc)
//!
//!   side_exit_bb_N:
//!     ; store regs[i] → reg_state[i]
//!     ret i64 exit_pc_N
//!
//!   clean_tail_bb:
//!     ; store regs[i] → reg_state[i]
//!     ret i64 head_pc
//! ```
//!
//! ## MVP whitelist
//!
//! Only depth-0 ops are supported. Bails on any inline depth > 0 or any
//! op outside: `LoadI, Move, Add, Sub, Mul, Mod, Lt, Le, Eq, Jmp`.
//!
//! ## Memory management
//!
//! The `EnginePair` holding the JIT mmap is parked in
//! `LlvmJitStorage::engines` via [`LlvmJitStorage::park_engine`] so the
//! trace function pointer stays valid for the Vm's lifetime.
//!
//! ## Known limitations (1K.H scope)
//!
//! - No `GetUpval` / `Call` / `GetTabUp` / `GetField` in trace
//! - No `ForLoop` / `ForPrep` / `TForCall`
//! - `per_exit_tags` uses clean-tail snapshot (conservative)
//! - `luna_jit_trace_materialize_frames` not wired (depth=0 only)

use crate::codegen::{declare_jit_helpers, finalize_module};
use crate::storage::{EnginePair, LlvmJitStorage};
use inkwell::IntPredicate;
use inkwell::context::Context;
use luna_core::jit::send_compat::{TArc, TCellBool, TCellPtr, TCellU32, TRefLock};
use luna_core::jit::trace_types::{
    CompileOptions, CompiledTrace, ExitTag, InlineSideExit, TagResKind, TraceFn, TraceRecord,
    classify_exit_tags,
};
use luna_core::vm::isa::Op;

/// Ops supported by the LLVM trace MVP.
fn is_mvp_trace_op(op: Op) -> bool {
    matches!(
        op,
        Op::LoadI
            | Op::Move
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Mod
            | Op::Lt
            | Op::Le
            | Op::Eq
            | Op::Jmp
    )
}

/// v2.1 Phase 1K.G — compile a `TraceRecord` to native code using LLVM.
///
/// Returns `None` when the record is outside the MVP whitelist (any op at
/// `inline_depth > 0`, any op not in the MVP set, or `!record.closed`).
/// The dispatcher treats `None` as "stay in interpreter".
pub(crate) fn try_compile_trace(
    storage: &mut LlvmJitStorage,
    record: &TraceRecord,
    _opts: CompileOptions,
) -> Option<CompiledTrace> {
    // Require a closed trace (has looped back to head_pc).
    if !record.closed {
        return None;
    }
    // MVP: only depth-0 ops.
    if record.ops.iter().any(|op| op.inline_depth > 0) {
        return None;
    }
    // MVP: only whitelisted ops.
    if record.ops.iter().any(|op| !is_mvp_trace_op(op.inst.op())) {
        return None;
    }

    let head_proto = record.head_proto;
    let max_stack = head_proto.max_stack as usize;
    let window_size = max_stack as u32;
    let head_pc = record.head_pc;
    let n = record.ops.len();

    // Determine which register slots are written by the trace → exit_tags.
    // Written slots get ExitTag::Int (all whitelisted producers write Int);
    // unwritten slots get ExitTag::Untouched (dispatcher restores from entry_tag).
    let mut written = vec![false; max_stack];
    for rop in &record.ops {
        let ins = rop.inst;
        match ins.op() {
            Op::LoadI | Op::Move | Op::Add | Op::Sub | Op::Mul | Op::Mod => {
                let a = ins.a() as usize;
                if a < max_stack {
                    written[a] = true;
                }
            }
            _ => {}
        }
    }
    let exit_tags_vec: Vec<ExitTag> = written
        .iter()
        .map(|&w| if w { ExitTag::Int } else { ExitTag::Untouched })
        .collect();
    let global_tag_res_kind: TagResKind = classify_exit_tags(&exit_tags_vec);
    let exit_tags: TArc<[ExitTag]> = exit_tags_vec.into();

    // Compile the LLVM trace function.
    let (entry_ptr, pair) = compile_trace_fn(record, max_stack, head_pc)?;

    // Park the engine so the JIT mmap stays alive for the Vm's lifetime.
    storage.park_engine(pair);

    // SAFETY: `entry_ptr` was produced by LLVM's JIT execution engine for a
    // function with the `TraceFn` signature (`unsafe extern "C" fn(*mut i64)
    // -> i64`). The engine (and thus the mcode page) is kept alive by
    // `LlvmJitStorage::engines` for the duration of the Vm that owns the
    // storage. The transmute merely labels the raw fn pointer with the
    // correct type; the mcode behind it satisfies the calling convention.
    let entry: TraceFn = unsafe { std::mem::transmute(entry_ptr) };

    // Build CompiledTrace with MVP defaults for all fields we don't
    // specialise at depth=0 / MVP op set.
    let empty_per_exit_tags: TArc<[(u32, TArc<[ExitTag]>)]> = vec![].into();
    let empty_per_exit_inline: TArc<[InlineSideExit]> = vec![].into();
    let exit_hit_counts: TArc<[TCellU32]> = vec![].into();
    let exit_side_trace_ptrs: TArc<[TCellPtr]> = vec![].into();
    let tags_side_trace_ptrs: TArc<[Box<TCellPtr>]> = vec![].into();
    let global_side_trace_ptr = Box::new(TCellPtr::new(std::ptr::null()));
    let entry_tags_arc: TArc<[u8]> = record.entry_tags.clone().into();

    Some(CompiledTrace {
        head_pc,
        entry,
        n_ops: n as u32,
        dispatchable: true,
        window_size,
        exit_tags,
        global_tag_res_kind,
        is_inline_abort_close: false,
        dispatch_off_reason: None,
        entry_tags: entry_tags_arc,
        per_exit_tags: empty_per_exit_tags,
        per_exit_inline: empty_per_exit_inline,
        exit_hit_counts,
        exit_side_trace_ptrs,
        tags_side_trace_ptrs,
        global_side_trace_ptr,
        side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
        has_any_side_wired: TCellBool::new(false),
        sinkable_sites_seen: 0,
        accum_bufferable_seen: 0,
        sunk_alloc_seen: 0,
        materialize_emit_count: 0,
        closure_seen: 0,
        body_writes: Box::from([]),
        downrec_link: None,
        downrec_multi_way_count: 0,
    })
}

/// Compile the trace body to LLVM IR and return `(entry_ptr, EnginePair)`.
///
/// The emitted function has signature `extern "C" fn(*mut i64) -> i64`:
/// - Arg 0: `reg_state` — pointer to a mutable i64 buffer of size `max_stack`.
/// - Return: `i64` — `head_pc` (clean tail) or a side-exit continuation PC.
fn compile_trace_fn(
    record: &TraceRecord,
    max_stack: usize,
    head_pc: u32,
) -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: same `Box::into_raw` + field-order drop discipline as the chunk
    // JIT; see `codegen::compile_compute_chunk` for the full rationale.
    // The `'static` is upheld by keeping `ctx_box` alive in the returned
    // `EnginePair::context` field (drop order: engine first, context second).
    let ctx: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };

    let module = ctx.create_module("luna_jit_llvm_trace");
    let builder = ctx.create_builder();
    let i64_type = ctx.i64_type();

    // Trace function: extern "C" fn(reg_state: *mut i64) -> i64.
    // Named "luna_jit_llvm_entry" to match the symbol `finalize_module` looks
    // for (each compile uses a fresh `Context`+`Module`, so no collision with
    // the chunk JIT's functions).
    let fn_type = i64_type.fn_type(&[i64_type.into()], false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);

    // Declare helpers (bound via `finalize_module`'s `bind_helper_symbols`
    // call). The MVP op set makes no helper calls, but declaring them upfront
    // mirrors the chunk JIT and keeps the symbol table consistent for future
    // 1K.H ops.
    let helpers = declare_jit_helpers(ctx, &module);

    // ── entry_bb: alloca register file + load from reg_state ──────────────
    let entry_bb = ctx.append_basic_block(function, "entry");
    let body_bb = ctx.append_basic_block(function, "body_loop");
    let clean_tail_bb = ctx.append_basic_block(function, "clean_tail");

    builder.position_at_end(entry_bb);

    let regs_ty = i64_type.array_type(max_stack as u32);
    let regs = builder.build_alloca(regs_ty, "regs").ok()?;

    // `reg_state` is arg 0, passed as i64 (pointer-as-integer via the
    // TraceFn ABI: `unsafe extern "C" fn(*mut i64) -> i64`).
    let rs_arg = function.get_nth_param(0)?.into_int_value();
    let ptr_type = ctx.ptr_type(inkwell::AddressSpace::default());
    let rs_ptr = builder.build_int_to_ptr(rs_arg, ptr_type, "rs_ptr").ok()?;

    // Load reg_state[i] into regs[i] for i in 0..max_stack.
    let zero = i64_type.const_zero();
    for i in 0..max_stack {
        let off = i64_type.const_int(i as u64, false);
        let src = unsafe {
            builder
                .build_in_bounds_gep(i64_type, rs_ptr, &[off], "rs_load_slot")
                .ok()?
        };
        let val = builder.build_load(i64_type, src, "rs_val").ok()?;
        let dst = unsafe {
            builder
                .build_in_bounds_gep(regs_ty, regs, &[zero, off], "reg_slot")
                .ok()?
        };
        builder.build_store(dst, val).ok()?;
    }
    builder.build_unconditional_branch(body_bb).ok()?;

    // ── body_loop_bb: emit recorded ops ───────────────────────────────────
    builder.position_at_end(body_bb);

    let code = &record.ops;
    let mut op_idx = 0usize;
    while op_idx < code.len() {
        let rop = &code[op_idx];
        let ins = rop.inst;

        // Helper: load from alloca slot `idx`.
        let load_reg = |idx: u32, name: &str| -> Option<inkwell::values::IntValue<'static>> {
            let off = i64_type.const_int(idx as u64, false);
            let slot = unsafe {
                builder
                    .build_in_bounds_gep(regs_ty, regs, &[zero, off], name)
                    .ok()?
            };
            let v = builder.build_load(i64_type, slot, name).ok()?;
            Some(v.into_int_value())
        };

        // Helper: store into alloca slot `idx`.
        let store_reg =
            |idx: u32, val: inkwell::values::IntValue<'static>, name: &str| -> Option<()> {
                let off = i64_type.const_int(idx as u64, false);
                let slot = unsafe {
                    builder
                        .build_in_bounds_gep(regs_ty, regs, &[zero, off], name)
                        .ok()?
                };
                builder.build_store(slot, val).ok()?;
                Some(())
            };

        match ins.op() {
            Op::LoadI => {
                let sbx = ins.sbx() as i64;
                let val = i64_type.const_int(sbx as u64, true);
                store_reg(ins.a(), val, "loadi_dst")?;
            }
            Op::Move => {
                let v = load_reg(ins.b(), "move_src")?;
                store_reg(ins.a(), v, "move_dst")?;
            }
            Op::Add => {
                let lhs = load_reg(ins.b(), "add_lhs")?;
                let rhs = load_reg(ins.c(), "add_rhs")?;
                let res = builder.build_int_add(lhs, rhs, "add_res").ok()?;
                store_reg(ins.a(), res, "add_dst")?;
            }
            Op::Sub => {
                let lhs = load_reg(ins.b(), "sub_lhs")?;
                let rhs = load_reg(ins.c(), "sub_rhs")?;
                let res = builder.build_int_sub(lhs, rhs, "sub_res").ok()?;
                store_reg(ins.a(), res, "sub_dst")?;
            }
            Op::Mul => {
                let lhs = load_reg(ins.b(), "mul_lhs")?;
                let rhs = load_reg(ins.c(), "mul_rhs")?;
                let res = builder.build_int_mul(lhs, rhs, "mul_res").ok()?;
                store_reg(ins.a(), res, "mul_dst")?;
            }
            Op::Mod => {
                // Lua floor-mod: sign of result matches divisor.
                let lhs = load_reg(ins.b(), "mod_lhs")?;
                let rhs = load_reg(ins.c(), "mod_rhs")?;
                let raw = builder.build_int_signed_rem(lhs, rhs, "mod_srem").ok()?;
                let zero_v = i64_type.const_zero();
                let nonzero = builder
                    .build_int_compare(IntPredicate::NE, raw, zero_v, "mod_nonzero")
                    .ok()?;
                let xor = builder.build_xor(raw, rhs, "mod_xor").ok()?;
                let sign_differ = builder
                    .build_int_compare(IntPredicate::SLT, xor, zero_v, "mod_signdif")
                    .ok()?;
                let need_fix = builder.build_and(nonzero, sign_differ, "mod_fix").ok()?;
                let fixed = builder.build_int_add(raw, rhs, "mod_fixed").ok()?;
                let res = builder
                    .build_select(need_fix, fixed, raw, "mod_res")
                    .ok()?
                    .into_int_value();
                store_reg(ins.a(), res, "mod_dst")?;
            }
            Op::Lt | Op::Le | Op::Eq => {
                // These ops are ALWAYS followed by a Jmp in the recorded ops.
                // Peeking ahead is safe; if there's no following op, bail.
                let jmp_op = code.get(op_idx + 1)?;
                if jmp_op.inst.op() != Op::Jmp {
                    return None;
                }

                // Comparison predicate.
                let pred = match ins.op() {
                    Op::Lt => IntPredicate::SLT,
                    Op::Le => IntPredicate::SLE,
                    Op::Eq => IntPredicate::EQ,
                    _ => unreachable!(),
                };
                let lhs = load_reg(ins.a(), "cmp_lhs")?;
                let rhs = load_reg(ins.b(), "cmp_rhs")?;
                let cmp = builder.build_int_compare(pred, lhs, rhs, "cmp_res").ok()?;

                // Jmp target formula: (jmp_pc + 1) + sj.
                let jmp_pc = jmp_op.pc as i64;
                let jmp_target_pc = (jmp_pc + 1 + jmp_op.inst.sj() as i64) as u32;
                let fall_pc = rop.pc + 2; // instruction after the Jmp

                // Lua 5.5 semantics: if `(cmp_result) == k` the Jmp is executed;
                // otherwise the Jmp is skipped.
                //
                // Since BOTH ops appear in the recorded trace, the Jmp WAS executed
                // → cmp_result == k at recording time. The compiled trace follows
                // that same path; a deviation at runtime fires the side exit.
                //
                // k=true  → recording: cmp==true  → jmp taken  → cont=jmp_target, exit=fall
                // k=false → recording: cmp==false → jmp taken  → cont=jmp_target, exit=fall
                //
                // Branch: if cmp==k at runtime → continue (jmp_target); else → side exit (fall).
                // Using the raw cmp i1:
                //   k=true  → true_bb=continue, false_bb=side  (cmp==true→cont)
                //   k=false → true_bb=side, false_bb=continue  (cmp==true→side)
                let continue_bb = ctx.append_basic_block(function, "cmp_cont");
                let side_exit_bb = ctx.append_basic_block(function, "side_exit");

                let (true_bb, false_bb) = if ins.k() {
                    (continue_bb, side_exit_bb)
                } else {
                    (side_exit_bb, continue_bb)
                };
                builder
                    .build_conditional_branch(cmp, true_bb, false_bb)
                    .ok()?;

                // Emit side_exit_bb: flush regs → reg_state, return exit_pc.
                builder.position_at_end(side_exit_bb);
                emit_store_back(&builder, i64_type, regs_ty, regs, rs_ptr, max_stack, zero)?;
                let exit_pc = if ins.k() { fall_pc } else { jmp_target_pc };
                let exit_pc_val = i64_type.const_int(exit_pc as u64, false);
                builder.build_return(Some(&exit_pc_val)).ok()?;

                // Continue emitting ops in continue_bb.
                builder.position_at_end(continue_bb);

                // Advance past the consumed Jmp.
                op_idx += 1;
            }
            Op::Jmp => {
                // In a closed trace the final back-edge Jmp returns to head_pc.
                // Emit a branch to clean_tail_bb and stop emitting body ops.
                builder.build_unconditional_branch(clean_tail_bb).ok()?;
                break;
            }
            _ => return None, // Not in MVP whitelist — caught by pre-check above.
        }
        op_idx += 1;
    }

    // If the while loop exited without an explicit break (no standalone Jmp at
    // the end), the current block has no terminator yet. Branch to clean_tail_bb.
    // `build_unconditional_branch` is idempotent when the block already has a
    // terminator (it would error and we'd propagate None), so we check the
    // terminator presence first.
    {
        let cur_bb = builder.get_insert_block()?;
        if cur_bb.get_terminator().is_none() {
            builder.build_unconditional_branch(clean_tail_bb).ok()?;
        }
    }

    // ── clean_tail_bb: flush regs → reg_state, return head_pc ─────────────
    builder.position_at_end(clean_tail_bb);
    emit_store_back(&builder, i64_type, regs_ty, regs, rs_ptr, max_stack, zero)?;
    let head_pc_val = i64_type.const_int(head_pc as u64, false);
    builder.build_return(Some(&head_pc_val)).ok()?;

    finalize_module(ctx_box, module, Some(&helpers))
}

/// Emit `store regs[i] → reg_state[i]` for `i in 0..max_stack` into the
/// builder's current block. Shared by the clean-tail and every side-exit BB.
fn emit_store_back(
    builder: &inkwell::builder::Builder<'static>,
    i64_type: inkwell::types::IntType<'static>,
    regs_ty: inkwell::types::ArrayType<'static>,
    regs: inkwell::values::PointerValue<'static>,
    rs_ptr: inkwell::values::PointerValue<'static>,
    max_stack: usize,
    zero: inkwell::values::IntValue<'static>,
) -> Option<()> {
    for i in 0..max_stack {
        let off = i64_type.const_int(i as u64, false);
        let src = unsafe {
            builder
                .build_in_bounds_gep(regs_ty, regs, &[zero, off], "sb_src")
                .ok()?
        };
        let val = builder.build_load(i64_type, src, "sb_val").ok()?;
        let dst = unsafe {
            builder
                .build_in_bounds_gep(i64_type, rs_ptr, &[off], "sb_dst")
                .ok()?
        };
        builder.build_store(dst, val).ok()?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `is_mvp_trace_op` accepts the expected op set and rejects
    /// others. Pins the whitelist without requiring a live LLVM context.
    #[test]
    fn mvp_whitelist_coverage() {
        for op in [
            Op::LoadI,
            Op::Move,
            Op::Add,
            Op::Sub,
            Op::Mul,
            Op::Mod,
            Op::Lt,
            Op::Le,
            Op::Eq,
            Op::Jmp,
        ] {
            assert!(is_mvp_trace_op(op), "{op:?} should be in MVP whitelist");
        }
        for op in [
            Op::Call,
            Op::TailCall,
            Op::GetUpval,
            Op::GetTabUp,
            Op::GetField,
            Op::Return0,
            Op::Return1,
            Op::LoadNil,
        ] {
            assert!(
                !is_mvp_trace_op(op),
                "{op:?} should NOT be in MVP whitelist"
            );
        }
    }
}
