//! Per-op IR emission for the compute path.

use crate::operands::{Operand, int_arith, is_arith};
use inkwell::context::Context;
use inkwell::values::{FunctionValue, PointerValue};
use luna_core::vm::isa::{Inst, Op};
use std::collections::HashMap;

/// Per-op emit context for the compute path. Holds the LLVM types
/// and the entry block's register-file alloca; each `emit_op` call
/// appends the op's IR sequence at the builder's current position.
///
/// `emit_op` handles non-control-flow ops (LoadI / LoadNil / Move and
/// the arithmetic ops). Control-flow ops (`Return0|Return1|Jmp` and
/// the comparisons) are handled in [`compile_compute_chunk`] directly
/// so the outer loop can switch BBs around the emitted terminator.
pub(super) struct ComputeEmitter<'ctx, 'a> {
    pub(super) ctx: &'ctx Context,
    pub(super) builder: &'a inkwell::builder::Builder<'ctx>,
    /// Held so the Op::Call lowerer can emit a
    /// `build_call` against the current entry `FunctionValue` for
    /// self-recursive shapes. Mirrors Cranelift's
    /// `module.declare_func_in_func(fn_id, bcx.func)` pattern.
    pub(super) function: FunctionValue<'ctx>,
    pub(super) i64_type: inkwell::types::IntType<'ctx>,
    pub(super) regs_ty: inkwell::types::ArrayType<'ctx>,
    pub(super) regs: PointerValue<'ctx>,
    /// `luna_jit_*` helper declarations.
    /// Op::GetUpval (ValueRead role) reaches for
    /// `helpers["luna_jit_upval_get"]`; future ops widen the call sites
    /// without per-op registration boilerplate.
    pub(super) helpers: &'a HashMap<&'static str, FunctionValue<'ctx>>,
    /// `luna_jit_helpers::self_call_desc` of this chunk's self calls
    pub(super) self_call_desc: i64,
    /// `llvm.read_register.i64`, which reads the stack pointer for the
    /// self-call guard.
    pub(super) read_register: FunctionValue<'ctx>,
    /// The registers holding nil at each self call (`flow::nils_at`).
    pub(super) nils_at: &'a [Option<Vec<bool>>],
    /// The registers a level hands to the interpreter.
    pub(super) num_regs: u32,
    /// The pc of the op being emitted.
    pub(super) pc: std::cell::Cell<usize>,
}

impl<'ctx, 'a> ComputeEmitter<'ctx, 'a> {
    /// Call `check(args)`; when it returns 0 hand this level to the
    /// interpreter at the op being emitted, else go on in a fresh block.
    pub(super) fn hand_unless(
        &self,
        check: FunctionValue<'ctx>,
        args: &[inkwell::values::BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Option<()> {
        let call = self
            .builder
            .build_call(check, args, &format!("{name}_check"))
            .ok()?;
        let ok = match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let zero = self.i64_type.const_zero();
        let is_ok = self
            .builder
            .build_int_compare(inkwell::IntPredicate::NE, ok, zero, &format!("{name}_ok"))
            .ok()?;
        let ok_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_go"));
        let deopt_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_deopt"));
        self.builder
            .build_conditional_branch(is_ok, ok_bb, deopt_bb)
            .ok()?;
        self.builder.position_at_end(deopt_bb);
        self.handoff_here()?;
        self.builder.position_at_end(ok_bb);
        Some(())
    }

    /// GEP the `idx`-th register slot inside the alloca.
    pub(super) fn reg_slot_ptr(&self, idx: u32, name: &str) -> Option<PointerValue<'ctx>> {
        let zero = self.i64_type.const_zero();
        let off = self.i64_type.const_int(idx as u64, false);
        // SAFETY: `regs` is an alloca of `plan.num_regs` slots, and
        // every `idx` passed here is below it: `ChunkPlan::from_proto`
        // sets `num_regs` to at least `num_params` (the prologue's
        // indices) and rejects a chunk whose reachable ops name a
        // register at or past it (`flow::registers_in_bounds`)
        unsafe {
            self.builder
                .build_in_bounds_gep(self.regs_ty, self.regs, &[zero, off], name)
                .ok()
        }
    }

    /// Store an i64 immediate into `regs[idx]`.
    pub(super) fn store_imm(&self, idx: u32, imm: i64) -> Option<()> {
        let slot = self.reg_slot_ptr(idx, "imm_slot")?;
        let val = self.i64_type.const_int(imm as u64, true);
        self.builder.build_store(slot, val).ok()?;
        Some(())
    }

    /// Load `regs[idx]` as an i64.
    pub(super) fn load_reg(&self, idx: u32, name: &str) -> Option<inkwell::values::IntValue<'ctx>> {
        let slot = self.reg_slot_ptr(idx, name)?;
        let v = self.builder.build_load(self.i64_type, slot, name).ok()?;
        Some(v.into_int_value())
    }

    /// An operand as an i64: a register load, or the constant itself.
    pub(super) fn operand_value(
        &self,
        operand: Operand,
        name: &str,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        match operand {
            Operand::Reg(idx) => self.load_reg(idx, name),
            Operand::Imm(imm) => Some(self.i64_type.const_int(imm as u64, true)),
        }
    }

    /// `R[A] = lhs <op> rhs` for the arithmetic ops (register,
    /// immediate and constant forms alike): wrapping i64 add / sub /
    /// mul, matching Lua's integer arithmetic
    /// (`math.maxinteger + 1 == math.mininteger`), and floor mod.
    ///
    /// No type-tag inspection: the compute whitelist has no op that
    /// produces a non-int value into a reg (LoadNil → 0, LoadI/Move →
    /// ints, arithmetic → int) and a constant operand is admitted only
    /// when it is an integer.
    pub(super) fn emit_arith(&self, ins: Inst, consts: &[Option<i64>]) -> Option<()> {
        let (op, lhs, rhs_operand) = int_arith(ins, consts)?;
        let lhs = self.operand_value(lhs, "arith_lhs")?;
        let rhs = self.operand_value(rhs_operand, "arith_rhs")?;
        let b = self.builder;
        let result = match op {
            Op::Add => b.build_int_add(lhs, rhs, "add_res").ok()?,
            Op::Sub => b.build_int_sub(lhs, rhs, "sub_res").ok()?,
            Op::Mul => b.build_int_mul(lhs, rhs, "mul_res").ok()?,
            Op::Mod => {
                // a constant divisor is never zero (`int_arith` refuses
                // one); a register one is checked here
                if matches!(rhs_operand, Operand::Reg(_)) {
                    self.deopt_if_zero(rhs)?;
                }
                self.floor_mod(lhs, rhs)?
            }
            _ => return None,
        };
        let dst = self.reg_slot_ptr(ins.a(), "arith_dst")?;
        b.build_store(dst, result).ok()?;
        Some(())
    }

    /// When `v` is zero, hand this level to the interpreter, which runs
    /// the op and raises the error (`n%0`).
    fn deopt_if_zero(&self, v: inkwell::values::IntValue<'ctx>) -> Option<()> {
        let zero = self.i64_type.const_zero();
        let is_zero = self
            .builder
            .build_int_compare(inkwell::IntPredicate::EQ, v, zero, "div_by_zero")
            .ok()?;
        let zero_bb = self.ctx.append_basic_block(self.function, "div_zero");
        let go_bb = self.ctx.append_basic_block(self.function, "div_go");
        self.builder
            .build_conditional_branch(is_zero, zero_bb, go_bb)
            .ok()?;
        self.builder.position_at_end(zero_bb);
        self.handoff_here()?;
        self.builder.position_at_end(go_bb);
        Some(())
    }

    /// Hand this level to the interpreter at the op being emitted.
    fn handoff_here(&self) -> Option<()> {
        let pc = self.pc.get();
        self.handoff(self.i64_type.const_int(pc as u64, false), pc)
    }

    /// Lua-semantic int mod (`lhs % rhs`).
    pub(super) fn floor_mod(
        &self,
        lhs: inkwell::values::IntValue<'ctx>,
        rhs: inkwell::values::IntValue<'ctx>,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        // Lua 5.4 / 5.5 `%` for two ints:
        //     R[A] = R[B] - floor(R[B] / R[C]) * R[C]
        // which differs from C's `%` (truncating remainder)
        // when the operand signs differ. Examples:
        //
        //   |  a |  b |  a % b (Lua) |  a srem b (C) |
        //   |----|----|--------------|---------------|
        //   |  7 |  3 |       1      |        1      |
        //   | -7 |  3 |       2      |       -1      |
        //   |  7 | -3 |      -2      |        1      |
        //   | -7 | -3 |      -1      |       -1      |
        //
        // LLVM's `srem` matches the C semantics, so we adjust:
        //     r = srem(a, b)
        //     r != 0  AND  (r ^ b) < 0   ⇒  r += b
        // (the "(r ^ b) < 0" test asks "do r and b have
        // different signs?"; combined with r != 0 it catches
        // exactly the rows above where Lua and C disagree.)
        //
        // Branch-free via `select`. The caller has ruled out a zero
        // divisor. `srem` traps on `mininteger % -1` (the quotient
        // overflows); any `x % -1` is 0, as is `x % 1`, so -1 is
        // replaced by 1.
        let minus_one = self.i64_type.const_all_ones();
        let one = self.i64_type.const_int(1, false);
        let is_minus_one = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                rhs,
                minus_one,
                "mod_by_minus_one",
            )
            .ok()?;
        let rhs = self
            .builder
            .build_select(is_minus_one, one, rhs, "mod_divisor")
            .ok()?
            .into_int_value();
        let raw = self
            .builder
            .build_int_signed_rem(lhs, rhs, "mod_srem")
            .ok()?;
        let zero = self.i64_type.const_zero();
        let nonzero = self
            .builder
            .build_int_compare(inkwell::IntPredicate::NE, raw, zero, "mod_raw_nonzero")
            .ok()?;
        // Sign-differ test: (raw XOR rhs) < 0 ↔ MSBs differ.
        let xor = self.builder.build_xor(raw, rhs, "mod_sign_xor").ok()?;
        let sign_differ = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "mod_sign_differ")
            .ok()?;
        let need_fix = self
            .builder
            .build_and(nonzero, sign_differ, "mod_need_fix")
            .ok()?;
        let fixed = self.builder.build_int_add(raw, rhs, "mod_fixed").ok()?;
        Some(
            self.builder
                .build_select(need_fix, fixed, raw, "mod_result")
                .ok()?
                .into_int_value(),
        )
    }

    pub(super) fn emit_op(&mut self, ins: Inst, consts: &[Option<i64>]) -> Option<()> {
        match ins.op() {
            Op::LoadI => {
                let a = ins.a();
                let sbx = ins.sbx() as i64;
                self.store_imm(a, sbx)?;
                Some(())
            }
            Op::LoadNil => {
                // `R[A..=A+B] = nil`. The compute path treats nil as
                // the i64 bit-pattern 0; that's a sound choice for
                // chunks that return ints (Return1 reads i64 directly)
                // because no recognised op observes nil as a distinct
                // tag. Adding bool/value-tagged ops requires switching
                // this to tagged bit patterns.
                let a = ins.a();
                let b = ins.b();
                for off in 0..=b {
                    self.store_imm(a + off, 0)?;
                }
                Some(())
            }
            Op::Move => {
                let a = ins.a();
                let b = ins.b();
                let v = self.load_reg(b, "move_src")?;
                let dst = self.reg_slot_ptr(a, "move_dst")?;
                self.builder.build_store(dst, v).ok()?;
                Some(())
            }
            op if is_arith(op) => self.emit_arith(ins, consts),
            _ => {
                // Whitelist guarded this in `ChunkPlan::from_proto`;
                // control-flow ops (Return0/Return1/Jmp and the
                // comparisons) are handled in `compile_compute_chunk`'s
                // outer loop, not here. Any other op slipping through =
                // someone added it to the whitelist without an
                // emit arm; bail rather than emit junk.
                None
            }
        }
    }

    /// `Op::GetUpval`: a value read goes through `luna_jit_upval_get`
    /// behind an integer check; a self-marker stores a placeholder 0.
    pub(super) fn emit_get_upval(&self, ins: Inst, value_read: bool) -> Option<()> {
        let builder = self.builder;
        let i64_type = self.i64_type;
        let dst = ins.a();
        if value_read {
            // ValueRead — the chunk computes with integers only,
            // so a non-integer upvalue returns at once with a
            // deopt parked (the interpreter re-runs the call);
            // otherwise luna_jit_upval_get(b) into regs[A].
            let idx_arg = i64_type.const_int(ins.b() as u64, false);
            let check = self.helpers.get("luna_jit_upval_is_int").copied()?;
            self.hand_unless(check, &[idx_arg.into()], "upv")?;
            let helper = self.helpers.get("luna_jit_upval_get").copied()?;
            let call_inst = builder
                .build_call(helper, &[idx_arg.into()], "upv_val")
                .ok()?;
            let v = match call_inst.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
                inkwell::values::ValueKind::Instruction(_) => return None,
            };
            let slot = self.reg_slot_ptr(dst, "upv_dst")?;
            builder.build_store(slot, v).ok()?;
        } else {
            // SelfMarker — destination register is never
            // read as a value (its only consumer is a
            // subsequent Op::Call which lowers as a direct
            // self-call). Store a placeholder 0 so the alloca
            // slot is initialised in case any path leaks a
            // read; mirrors Cranelift's `aligned_def(..., 0)`.
            let zero = i64_type.const_zero();
            let slot = self.reg_slot_ptr(dst, "upv_self_marker")?;
            builder.build_store(slot, zero).ok()?;
        }
        Some(())
    }

    /// Self-recursive `Op::Call`: a direct call to the entry function,
    /// result into regs[A], then return early if a deopt was parked.
    pub(super) fn emit_self_call(&self, ins: Inst, pc: usize) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "call_arg")?;
            arg_vals.push(v.into());
        }
        let v = self.guarded_self_call(&arg_vals, a, None, "self_call")?;
        // a call below that failed returns at once; one that went to the
        // interpreter takes this level with it
        self.after_self_call(pc, false)?;
        let slot = self.reg_slot_ptr(a, "call_dst")?;
        builder.build_store(slot, v).ok()?;
        Some(())
    }

    /// Self-recursive `Op::TailCall`: call the entry function and return
    /// its result (`Op::Call` + `Op::Return1` fused).
    pub(super) fn emit_self_tail_call(&self, ins: Inst, pc: usize) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "tail_arg")?;
            arg_vals.push(v.into());
        }
        let v = self.guarded_self_call(&arg_vals, a, Some(pc), "tail_call")?;
        // the callee took this level: one that went to the interpreter
        // took it there already
        self.after_self_call(pc, true)?;
        // Return the tail call result directly — no regs[A] store.
        builder.build_return(Some(&v)).ok()?;
        Some(())
    }

    /// After the self call at `pc`: with the context's flag set, return at
    /// once (a call failed, or with `tail` the callee went to the
    /// interpreter as this level), or hand this level to the interpreter,
    /// going on after the call (the callee went there) or with it (the
    /// callee did not start).
    fn after_self_call(&self, pc: usize, tail: bool) -> Option<()> {
        use luna_jit_helpers::{SELF_HANDED, SELF_NOT_STARTED};
        let builder = self.builder;
        let i64t = self.i64_type;
        let flag = self.ctx_word(1, "call_flag")?;
        let set = builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                flag,
                i64t.const_zero(),
                "flag_set",
            )
            .ok()?;
        let cold = self.ctx.append_basic_block(self.function, "flag_cold");
        let go = self.ctx.append_basic_block(self.function, "flag_go");
        builder.build_conditional_branch(set, cold, go).ok()?;
        builder.position_at_end(cold);
        let hand_at = |what: i64| {
            builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    flag,
                    i64t.const_int(what as u64, false),
                    "flag_is",
                )
                .ok()
        };
        let not_started = hand_at(SELF_NOT_STARTED)?;
        let handed = hand_at(SELF_HANDED)?;
        let hand = if tail {
            not_started
        } else {
            builder.build_or(not_started, handed, "hand").ok()?
        };
        let hand_bb = self.ctx.append_basic_block(self.function, "hand");
        let ret_bb = self.ctx.append_basic_block(self.function, "flag_ret");
        builder
            .build_conditional_branch(hand, hand_bb, ret_bb)
            .ok()?;
        builder.position_at_end(ret_bb);
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        builder.position_at_end(hand_bb);
        let at = i64t.const_int(pc as u64, false);
        let after = i64t.const_int(pc as u64 + 1, false);
        let resume = builder
            .build_select(not_started, at, after, "resume")
            .ok()?;
        self.handoff(resume.into_int_value(), pc)?;
        builder.position_at_end(go);
        Some(())
    }

    /// Hand this level to the interpreter, which goes on with it at `pc`
    /// (a value), its registers as they are before the self call at
    /// `call_pc` (`luna_jit_handoff_frame`), and return.
    fn handoff(&self, pc: inkwell::values::IntValue<'ctx>, call_pc: usize) -> Option<()> {
        // (an upvalue check failing parked a deopt the record replaces)
        use luna_core::runtime::value::raw;
        let builder = self.builder;
        let i64t = self.i64_type;
        let n = self.num_regs;
        let words = i64t.array_type(2 * n);
        let buf = builder.build_alloca(words, "handoff_regs").ok()?;
        let nils = self.nils_at.get(call_pc).cloned().flatten()?;
        for r in 0..n {
            let nil = nils.get(r as usize).copied().unwrap_or(true);
            let tag = if nil { raw::NIL } else { raw::INT };
            let at = |w: u32, name: &str| {
                // SAFETY: `buf` is an alloca of `2 * n` words and `w < 2 * n`
                unsafe {
                    builder.build_in_bounds_gep(
                        words,
                        buf,
                        &[i64t.const_zero(), i64t.const_int(u64::from(w), false)],
                        name,
                    )
                }
                .ok()
            };
            builder
                .build_store(at(2 * r, "tag_at")?, i64t.const_int(u64::from(tag), false))
                .ok()?;
            if !nil {
                let v = self.load_reg(r, "handoff_reg")?;
                builder.build_store(at(2 * r + 1, "val_at")?, v).ok()?;
            }
        }
        let addr = builder.build_ptr_to_int(buf, i64t, "handoff_addr").ok()?;
        let base = self.function.get_nth_param(2)?.into_int_value();
        let record = self.helpers.get("luna_jit_handoff_frame").copied()?;
        builder
            .build_call(
                record,
                &[
                    pc.into(),
                    base.into(),
                    addr.into(),
                    i64t.const_int(u64::from(n), false).into(),
                ],
                "handoff",
            )
            .ok()?;
        let flag_at = self.ctx_addr(1, "flag_at")?;
        let handed = i64t.const_int(luna_jit_helpers::SELF_HANDED as u64, false);
        builder.build_store(flag_at, handed).ok()?;
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        Some(())
    }

    /// The address of word `w` of the self-call context (the body's first
    /// parameter).
    fn ctx_addr(&self, w: u64, name: &str) -> Option<inkwell::values::PointerValue<'ctx>> {
        let base = self.function.get_nth_param(0)?.into_int_value();
        let off = self.i64_type.const_int(8 * w, false);
        let at = self.builder.build_int_add(base, off, name).ok()?;
        self.builder
            .build_int_to_ptr(at, self.ctx.ptr_type(Default::default()), name)
            .ok()
    }

    fn ctx_word(&self, w: u64, name: &str) -> Option<inkwell::values::IntValue<'ctx>> {
        let p = self.ctx_addr(w, name)?;
        Some(
            self.builder
                .build_load(self.i64_type, p, name)
                .ok()?
                .into_int_value(),
        )
    }

    /// Return 0 from the chunk unless `ok`, else go on in a fresh block.
    fn return_if_not(&self, ok: inkwell::values::IntValue<'ctx>, name: &str) -> Option<()> {
        let go = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_go"));
        let back = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_back"));
        self.builder.build_conditional_branch(ok, go, back).ok()?;
        self.builder.position_at_end(back);
        self.builder
            .build_return(Some(&self.i64_type.const_zero()))
            .ok()?;
        self.builder.position_at_end(go);
        Some(())
    }

    /// A self call: the body itself, at the level above this one's
    /// register `a`. The body checks on entry whether it may run natively
    /// and counts its level when it must (`emit_self_guard`), so the call
    /// is a plain call LLVM can turn into a loop. A tail call reuses this
    /// level; PUC places the callee above it first, which is counted here
    /// (`luna_jit_self_level` with [`SELF_LEVEL_TAIL`]).
    fn guarded_self_call(
        &self,
        args: &[inkwell::values::BasicMetadataValueEnum<'ctx>],
        a: u32,
        tail: Option<usize>,
        name: &str,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let base = self.function.get_nth_param(2)?.into_int_value();
        let depth = self.function.get_nth_param(3)?.into_int_value();
        let up = builder
            .build_int_add(base, i64t.const_int(u64::from(a) + 1, false), "up_base")
            .ok()?;
        let deeper = builder
            .build_int_add(depth, i64t.const_int(1, false), "up_depth")
            .ok()?;
        let (base, depth) = if let Some(pc) = tail {
            let desc = self.self_call_desc | luna_jit_helpers::SELF_LEVEL_TAIL;
            self.count_level(up, deeper, desc, Some(pc), name)?;
            (base, depth)
        } else {
            (up, deeper)
        };
        let mut body_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            self.function.get_nth_param(0)?.into(),
            self.function.get_nth_param(1)?.into(),
            base.into(),
            depth.into(),
        ];
        body_args.extend_from_slice(args);
        let call = builder.build_call(self.function, &body_args, name).ok()?;
        call.set_call_convention(self.function.get_call_conventions());
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => Some(bv.into_int_value()),
            inkwell::values::ValueKind::Instruction(_) => None,
        }
    }

    /// The level at `base` and `depth` counted when it reaches the limits
    /// in the context (`luna_jit_self_level`); return 0 when that fails.
    fn count_level(
        &self,
        base: inkwell::values::IntValue<'ctx>,
        depth: inkwell::values::IntValue<'ctx>,
        desc: i64,
        tail: Option<usize>,
        name: &str,
    ) -> Option<()> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let base_limit = self.ctx_word(2, "base_limit")?;
        let cross = builder
            .build_int_compare(inkwell::IntPredicate::SGE, base, base_limit, "high")
            .ok()?;
        let count_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_level"));
        let go_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_level_go"));
        builder
            .build_conditional_branch(cross, count_bb, go_bb)
            .ok()?;
        builder.position_at_end(count_bb);
        let level_fn = self.helpers.get("luna_jit_self_level").copied()?;
        let ctx_arg = self.function.get_nth_param(0)?.into_int_value();
        let r = builder
            .build_call(
                level_fn,
                &[
                    ctx_arg.into(),
                    i64t.const_int(desc as u64, false).into(),
                    base.into(),
                    depth.into(),
                ],
                "level",
            )
            .ok()?;
        let r = match r.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let ok = builder
            .build_int_compare(inkwell::IntPredicate::EQ, r, i64t.const_zero(), "level_ok")
            .ok()?;
        match tail {
            // the tail call did not start: this level goes with it
            Some(pc) => {
                let failed_bb = self
                    .ctx
                    .append_basic_block(self.function, &format!("{name}_level_failed"));
                builder
                    .build_conditional_branch(ok, go_bb, failed_bb)
                    .ok()?;
                builder.position_at_end(failed_bb);
                self.after_self_call(pc, true)?;
                builder.build_return(Some(&i64t.const_zero())).ok()?;
            }
            None => {
                self.return_if_not(ok, &format!("{name}_level"))?;
                builder.build_unconditional_branch(go_bb).ok()?;
            }
        }
        builder.position_at_end(go_bb);
        Some(())
    }

    /// At the body's start, when it was entered by a self call: with the
    /// native stack below the context's limit, leave this call to the
    /// interpreter (the caller hands itself over), as the Cranelift tier's
    /// stub does; otherwise count the level when it must be.
    pub(super) fn emit_self_guard(&self) -> Option<()> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let limit = self.function.get_nth_param(1)?.into_int_value();
        let base = self.function.get_nth_param(2)?.into_int_value();
        let depth = self.function.get_nth_param(3)?.into_int_value();
        // x86_64 or aarch64, the architectures `lib.rs` admits
        let sp_name = if cfg!(target_arch = "x86_64") {
            "rsp"
        } else {
            "sp"
        };
        let sp_reg = self
            .ctx
            .metadata_node(&[self.ctx.metadata_string(sp_name).into()]);
        let sp = builder
            .build_call(self.read_register, &[sp_reg.into()], "sp")
            .ok()?;
        let sp = match sp.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let slow = builder
            .build_int_compare(inkwell::IntPredicate::ULT, sp, limit, "stack_low")
            .ok()?;
        let slow_bb = self.ctx.append_basic_block(self.function, "self_slow");
        let go_bb = self.ctx.append_basic_block(self.function, "self_native");
        builder
            .build_conditional_branch(slow, slow_bb, go_bb)
            .ok()?;
        builder.position_at_end(slow_bb);
        // the interpreter makes the call
        let flag_at = self.ctx_addr(1, "flag_at")?;
        let not_started = i64t.const_int(luna_jit_helpers::SELF_NOT_STARTED as u64, false);
        builder.build_store(flag_at, not_started).ok()?;
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        builder.position_at_end(go_bb);
        self.count_level(base, depth, self.self_call_desc, None, "entry")
    }
}
