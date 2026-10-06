//! One recorded instruction as LLVM IR, with Cranelift's semantics.

use super::*;
use inkwell::values::BasicValueEnum;
use inkwell::{FloatPredicate, IntPredicate};

fn int_pred(cc: IntCC) -> IntPredicate {
    match cc {
        IntCC::Equal => IntPredicate::EQ,
        IntCC::NotEqual => IntPredicate::NE,
        IntCC::SignedLessThan => IntPredicate::SLT,
        IntCC::SignedGreaterThanOrEqual => IntPredicate::SGE,
        IntCC::SignedGreaterThan => IntPredicate::SGT,
        IntCC::SignedLessThanOrEqual => IntPredicate::SLE,
        IntCC::UnsignedLessThan => IntPredicate::ULT,
        IntCC::UnsignedGreaterThanOrEqual => IntPredicate::UGE,
        IntCC::UnsignedGreaterThan => IntPredicate::UGT,
        IntCC::UnsignedLessThanOrEqual => IntPredicate::ULE,
    }
}

fn float_pred(cc: FloatCC) -> FloatPredicate {
    match cc {
        FloatCC::Ordered => FloatPredicate::ORD,
        FloatCC::Unordered => FloatPredicate::UNO,
        FloatCC::Equal => FloatPredicate::OEQ,
        FloatCC::NotEqual => FloatPredicate::UNE,
        FloatCC::OrderedNotEqual => FloatPredicate::ONE,
        FloatCC::UnorderedOrEqual => FloatPredicate::UEQ,
        FloatCC::LessThan => FloatPredicate::OLT,
        FloatCC::LessThanOrEqual => FloatPredicate::OLE,
        FloatCC::GreaterThan => FloatPredicate::OGT,
        FloatCC::GreaterThanOrEqual => FloatPredicate::OGE,
        FloatCC::UnorderedOrLessThan => FloatPredicate::ULT,
        FloatCC::UnorderedOrLessThanOrEqual => FloatPredicate::ULE,
        FloatCC::UnorderedOrGreaterThan => FloatPredicate::UGT,
        FloatCC::UnorderedOrGreaterThanOrEqual => FloatPredicate::UGE,
    }
}

impl<'c> Gen<'c, '_> {
    /// A comparison's `i1` as the `i8` 0 / 1 Cranelift gives.
    fn flag(&self, c: IntValue<'c>) -> R<IntValue<'c>> {
        b(self.b.build_int_z_extend(c, self.ctx.i8_type(), ""))
    }

    /// Value `n` (an integer of any width) taken as a condition.
    fn cond(&self, n: u32) -> R<IntValue<'c>> {
        let x = self.iv(n);
        let zero = x.get_type().const_zero();
        b(self.b.build_int_compare(IntPredicate::NE, x, zero, ""))
    }

    /// `x op y`; `y` may be narrower or wider than `x` only for shifts.
    fn bin(
        &self,
        op: BinOp,
        x: BasicValueEnum<'c>,
        y: BasicValueEnum<'c>,
    ) -> R<BasicValueEnum<'c>> {
        let bb = self.b;
        if let (BasicValueEnum::FloatValue(x), BasicValueEnum::FloatValue(y)) = (x, y) {
            return self.float_bin(op, x, y);
        }
        let (x, y) = (x.into_int_value(), y.into_int_value());
        Ok(match op {
            BinOp::Add => b(bb.build_int_add(x, y, ""))?,
            BinOp::Sub => b(bb.build_int_sub(x, y, ""))?,
            BinOp::Mul => b(bb.build_int_mul(x, y, ""))?,
            // the lowerer only divides by a divisor it has checked (or by a
            // constant other than 0 and -1)
            BinOp::Sdiv => b(bb.build_int_signed_div(x, y, ""))?,
            BinOp::Umulhi => {
                let t = x.get_type();
                let wide = self.ctx.i128_type();
                let xw = b(bb.build_int_z_extend(x, wide, ""))?;
                let yw = b(bb.build_int_z_extend(y, wide, ""))?;
                let p = b(bb.build_int_mul(xw, yw, ""))?;
                let sh = wide.const_int(u64::from(t.get_bit_width()), false);
                let hi = b(bb.build_right_shift(p, sh, false, ""))?;
                b(bb.build_int_truncate(hi, t, ""))?
            }
            BinOp::Smin | BinOp::Smax => {
                let name = if op == BinOp::Smin {
                    "llvm.smin"
                } else {
                    "llvm.smax"
                };
                let f = self.intrinsic(name, &[x.get_type().into()])?;
                let r = b(bb.build_call(f, &[x.into(), y.into()], ""))?;
                return match r.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => Ok(v),
                    _ => Err("llvm:intrinsic"),
                };
            }
            BinOp::And => b(bb.build_and(x, y, ""))?,
            BinOp::Or => b(bb.build_or(x, y, ""))?,
            BinOp::Xor => b(bb.build_xor(x, y, ""))?,
            BinOp::Shl | BinOp::Ushr | BinOp::Sshr => {
                // Cranelift shifts by the amount modulo the width
                let t = x.get_type();
                let w = u64::from(t.get_bit_width());
                let y = match y.get_type().get_bit_width().cmp(&t.get_bit_width()) {
                    std::cmp::Ordering::Less => b(bb.build_int_z_extend(y, t, ""))?,
                    std::cmp::Ordering::Greater => b(bb.build_int_truncate(y, t, ""))?,
                    std::cmp::Ordering::Equal => y,
                };
                let y = b(bb.build_and(y, t.const_int(w - 1, false), ""))?;
                match op {
                    BinOp::Shl => b(bb.build_left_shift(x, y, ""))?,
                    BinOp::Ushr => b(bb.build_right_shift(x, y, false, ""))?,
                    _ => b(bb.build_right_shift(x, y, true, ""))?,
                }
            }
            BinOp::Fadd | BinOp::Fsub | BinOp::Fmul | BinOp::Fdiv => {
                return Err("llvm:float-operands");
            }
        }
        .into())
    }

    /// `x op y` as the machine computes it. Plain LLVM float operations
    /// fold on constants to a NaN of LLVM's choosing (positive), where
    /// x86 makes a negative one, and Lua prints the sign. LLVM folds a
    /// constrained operation that raises invalid only when exceptions are
    /// not strict.
    fn float_bin(&self, op: BinOp, x: FloatValue<'c>, y: FloatValue<'c>) -> R<BasicValueEnum<'c>> {
        let name = match op {
            BinOp::Fadd => "llvm.experimental.constrained.fadd",
            BinOp::Fsub => "llvm.experimental.constrained.fsub",
            BinOp::Fmul => "llvm.experimental.constrained.fmul",
            BinOp::Fdiv => "llvm.experimental.constrained.fdiv",
            _ => return Err("llvm:float-operands"),
        };
        let f = self.intrinsic(name, &[x.get_type().into()])?;
        let round = self.ctx.metadata_string("round.tonearest");
        let except = self.ctx.metadata_string("fpexcept.strict");
        let call = b(self
            .b
            .build_call(f, &[x.into(), y.into(), round.into(), except.into()], ""))?;
        call.add_attribute(inkwell::attributes::AttributeLoc::Function, self.strictfp());
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Ok(v),
            _ => Err("llvm:intrinsic"),
        }
    }

    fn float_call(&self, name: &str, x: FloatValue<'c>) -> R<BasicValueEnum<'c>> {
        let f = self.intrinsic(name, &[x.get_type().into()])?;
        match b(self.b.build_call(f, &[x.into()], ""))?.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => Ok(v),
            _ => Err("llvm:intrinsic"),
        }
    }

    fn un(&self, u: UnOp, i: &Inst) -> R<BasicValueEnum<'c>> {
        let bb = self.b;
        let from = self.lir.value_ty[i.a as usize];
        let x = self.v(i.a);
        Ok(match u {
            UnOp::Ineg => b(bb.build_int_neg(x.into_int_value(), ""))?.into(),
            UnOp::Bnot => b(bb.build_not(x.into_int_value(), ""))?.into(),
            UnOp::Fneg => b(bb.build_float_neg(x.into_float_value(), ""))?.into(),
            UnOp::Floor => self.float_call("llvm.floor", x.into_float_value())?,
            UnOp::Ceil => self.float_call("llvm.ceil", x.into_float_value())?,
            UnOp::Uextend | UnOp::Ireduce if from == i.ty => x,
            UnOp::Uextend => {
                b(bb.build_int_z_extend(x.into_int_value(), self.ity(i.ty), ""))?.into()
            }
            UnOp::Ireduce => {
                b(bb.build_int_truncate(x.into_int_value(), self.ity(i.ty), ""))?.into()
            }
            UnOp::Bitcast if from == i.ty => x,
            UnOp::Bitcast => b(bb.build_bit_cast(x, self.ty(i.ty), ""))?,
            UnOp::FcvtFromSint => {
                b(bb.build_signed_int_to_float(x.into_int_value(), self.ctx.f64_type(), ""))?.into()
            }
            // the lowerer range-checks the operand first
            UnOp::FcvtToSint => {
                b(bb.build_float_to_signed_int(x.into_float_value(), self.ity(i.ty), ""))?.into()
            }
            UnOp::FcvtToSintSat => {
                let f = self.intrinsic(
                    "llvm.fptosi.sat",
                    &[self.ty(i.ty), self.ctx.f64_type().into()],
                )?;
                match b(bb.build_call(f, &[x.into()], ""))?.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(v) => v,
                    _ => return Err("llvm:intrinsic"),
                }
            }
        })
    }

    fn load(&self, ty: BasicTypeEnum<'c>, p: PointerValue<'c>) -> R<BasicValueEnum<'c>> {
        let v = b(self.b.build_load(ty, p, ""))?;
        if let Some(inst) = v.as_instruction_value() {
            inst.set_alignment(1).map_err(|_| "llvm:builder")?;
        }
        Ok(v)
    }

    fn store(&self, v: BasicValueEnum<'c>, p: PointerValue<'c>) -> R<()> {
        let inst = b(self.b.build_store(p, v))?;
        inst.set_alignment(1).map_err(|_| "llvm:builder")
    }

    pub(super) fn inst(&mut self, i: &Inst) -> R<()> {
        let bb = self.b;
        match i.op {
            Op::Iconst(n) => {
                let v = self.ity(i.ty).const_int(n as u64, false);
                self.set(i.dst, v);
            }
            Op::Reloc(n) => {
                let live = self.relocs[n as usize].1;
                let v = self.ctx.i64_type().const_int(live as u64, false);
                self.set(i.dst, v);
            }
            Op::Fconst(bits) => {
                let k = self.ctx.i64_type().const_int(bits, false);
                let v = b(bb.build_bit_cast(k, self.ctx.f64_type(), ""))?;
                self.set(i.dst, v);
            }
            Op::Bin(op) => {
                let v = self.bin(op, self.v(i.a), self.v(i.b))?;
                self.set(i.dst, v);
            }
            Op::BinImm(op, imm) => {
                let c = self.ity(i.ty).const_int(imm as u64, false);
                let v = self.bin(op, self.v(i.a), c.into())?;
                self.set(i.dst, v);
            }
            Op::Un(u) => {
                let v = self.un(u, i)?;
                self.set(i.dst, v);
            }
            Op::Icmp(cc) => {
                let c = b(bb.build_int_compare(int_pred(cc), self.iv(i.a), self.iv(i.b), ""))?;
                let v = self.flag(c)?;
                self.set(i.dst, v);
            }
            Op::IcmpImm(cc, imm) => {
                let x = self.iv(i.a);
                let k = x.get_type().const_int(imm as u64, false);
                let c = b(bb.build_int_compare(int_pred(cc), x, k, ""))?;
                let v = self.flag(c)?;
                self.set(i.dst, v);
            }
            Op::Fcmp(cc) => {
                let c = b(bb.build_float_compare(float_pred(cc), self.fv(i.a), self.fv(i.b), ""))?;
                let v = self.flag(c)?;
                self.set(i.dst, v);
            }
            Op::Select => {
                let c = self.cond(i.a)?;
                let v = b(bb.build_select(c, self.v(i.b), self.v(i.c), ""))?;
                self.set(i.dst, v);
            }
            Op::Load(off) => {
                let p = self.addr(self.iv(i.a), off)?;
                let v = self.load(self.ty(i.ty), p)?;
                self.set(i.dst, v);
            }
            Op::Uload8(off) => {
                let p = self.addr(self.iv(i.a), off)?;
                let v = self.load(self.ctx.i8_type().into(), p)?.into_int_value();
                let v = if i.ty == Ty::I8 {
                    v
                } else {
                    b(bb.build_int_z_extend(v, self.ity(i.ty), ""))?
                };
                self.set(i.dst, v);
            }
            Op::Store(off) => {
                let p = self.addr(self.iv(i.b), off)?;
                self.store(self.v(i.a), p)?;
            }
            Op::StackAddr(slot, off) => {
                let v = self.slot_addr(slot, off)?;
                self.set(i.dst, v);
            }
            Op::StackLoad(slot, off) => {
                let p = self.addr(self.slot_addr(slot, off)?, 0)?;
                let v = self.load(self.ty(i.ty), p)?;
                self.set(i.dst, v);
            }
            Op::StackStore(slot, off) => {
                let p = self.addr(self.slot_addr(slot, off)?, 0)?;
                self.store(self.v(i.a), p)?;
            }
            Op::Jump => {
                let args = self.args(i);
                let to = self.edge(i.a, &args)?;
                b(bb.build_unconditional_branch(to))?;
            }
            Op::Brif(n_then) => self.brif(i, n_then)?,
            // only the baseline tier counts iterations
            Op::TierCount { .. } => {
                let to = self.edge(i.c, &[])?;
                b(bb.build_unconditional_branch(to))?;
            }
            Op::Call | Op::CallIndirect => self.call(i)?,
            Op::Return => {
                let v = self.v(i.a);
                b(bb.build_return(Some(&v)))?;
            }
            Op::VarRead => {
                let home = self.vars[i.a as usize];
                let v = b(bb.build_load(self.ty(self.lir.var_ty[i.a as usize]), home, ""))?;
                self.set(i.dst, v);
            }
            Op::VarWrite => {
                b(bb.build_store(self.vars[i.a as usize], self.v(i.b)))?;
            }
        }
        Ok(())
    }

    /// A two-way branch. Both ways into one block go through a block of
    /// their own on the else side, since a phi takes one value per
    /// predecessor.
    fn brif(&mut self, i: &Inst, n_then: u32) -> R<()> {
        let args = self.args(i);
        let (ta, ea) = args.split_at(n_then as usize);
        let c = self.cond(i.a)?;
        let then_bb = self.edge(i.b, ta)?;
        if i.b != i.c {
            let else_bb = self.edge(i.c, ea)?;
            b(self.b.build_conditional_branch(c, then_bb, else_bb))?;
            return Ok(());
        }
        let here = self.b.get_insert_block().ok_or("llvm:builder")?;
        let split = self.ctx.insert_basic_block_after(here, "");
        b(self.b.build_conditional_branch(c, then_bb, split))?;
        self.b.position_at_end(split);
        let else_bb = self.edge(i.c, ea)?;
        b(self.b.build_unconditional_branch(else_bb))?;
        Ok(())
    }
}
