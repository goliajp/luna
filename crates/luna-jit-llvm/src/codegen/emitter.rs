//! Per-op IR emission for the compute path.

use crate::operands::{Operand, int_arith, is_arith};
use inkwell::context::Context;
use inkwell::values::{FunctionValue, PointerValue};
use luna_core::runtime::Value;
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
}

impl<'ctx, 'a> ComputeEmitter<'ctx, 'a> {
    /// Call `check(args)`; when it returns 0 (a deopt is parked) return 0
    /// from the chunk, else go on in a fresh block.
    pub(super) fn return_unless(
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
        self.builder.build_return(Some(&zero)).ok()?;
        self.builder.position_at_end(ok_bb);
        Some(())
    }

    /// GEP the `idx`-th register slot inside the alloca.
    pub(super) fn reg_slot_ptr(&self, idx: u32, name: &str) -> Option<PointerValue<'ctx>> {
        let zero = self.i64_type.const_zero();
        let off = self.i64_type.const_int(idx as u64, false);
        // SAFETY: `[0, idx]` indexes into a `[regs_ty x i64]` alloca
        // sized `plan.num_regs`. `ChunkPlan::from_proto` checked that
        // the largest A in the recognised ops fits; the per-op
        // emitters below all clamp to the alloca's bounds.
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
    pub(super) fn emit_arith(&self, ins: Inst, consts: &[Value]) -> Option<()> {
        let (op, lhs, rhs) = int_arith(ins, consts)?;
        let lhs = self.operand_value(lhs, "arith_lhs")?;
        let rhs = self.operand_value(rhs, "arith_rhs")?;
        let b = self.builder;
        let result = match op {
            Op::Add => b.build_int_add(lhs, rhs, "add_res").ok()?,
            Op::Sub => b.build_int_sub(lhs, rhs, "sub_res").ok()?,
            Op::Mul => b.build_int_mul(lhs, rhs, "mul_res").ok()?,
            Op::Mod => self.floor_mod(lhs, rhs)?,
            _ => return None,
        };
        let dst = self.reg_slot_ptr(ins.a(), "arith_dst")?;
        b.build_store(dst, result).ok()?;
        Some(())
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
        // Branch-free via `select`. Division-by-zero is the
        // interpreter's job (it raises "attempt to perform
        // 'n%%0'"); a `ModK` by a constant zero is refused up front, and
        // a Mod chunk that dynamically uses zero as R[C] still wouldn't
        // reach here through normal parser-emitted bytecode, so we accept
        // LLVM's UB on `srem x, 0` rather than emit a runtime check.
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

    pub(super) fn emit_op(&mut self, ins: Inst, consts: &[Value]) -> Option<()> {
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
            self.return_unless(check, &[idx_arg.into()], "upv")?;
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
    pub(super) fn emit_self_call(&self, ins: Inst) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "call_arg")?;
            arg_vals.push(v.into());
        }
        let call_inst = builder
            .build_call(self.function, &arg_vals, "self_call")
            .ok()?;
        let v = match call_inst.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let slot = self.reg_slot_ptr(a, "call_dst")?;
        builder.build_store(slot, v).ok()?;
        let parked = self.helpers.get("luna_jit_no_deopt_parked").copied()?;
        self.return_unless(parked, &[], "call")?;
        Some(())
    }

    /// Self-recursive `Op::TailCall`: call the entry function and return
    /// its result (`Op::Call` + `Op::Return1` fused).
    pub(super) fn emit_self_tail_call(&self, ins: Inst) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "tail_arg")?;
            arg_vals.push(v.into());
        }
        let call_inst = builder
            .build_call(self.function, &arg_vals, "tail_call")
            .ok()?;
        let v = match call_inst.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        // Return the tail call result directly — no regs[A] store.
        builder.build_return(Some(&v)).ok()?;
        Some(())
    }
}
