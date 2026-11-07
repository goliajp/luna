//! One recorded instruction as LLVM IR, with Cranelift's semantics.

use super::preds::{float_pred, int_pred};
use super::*;
use inkwell::IntPredicate;
use inkwell::values::BasicValueEnum;

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
            BinOp::Udiv => b(bb.build_int_unsigned_div(x, y, ""))?,
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
            Op::Load(_)
            | Op::Uload8(_)
            | Op::Store(_)
            | Op::StackAddr(..)
            | Op::StackLoad(..)
            | Op::StackStore(..)
            | Op::VarRead
            | Op::VarWrite => self.mem(i)?,
            _ => self.control(i)?,
        }
        Ok(())
    }

    /// A branch, a call or a return.
    fn control(&mut self, i: &Inst) -> R<()> {
        let bb = self.b;
        match i.op {
            Op::Jump => {
                let args = self.args(i);
                let to = self.edge(i.a, &args)?;
                b(bb.build_unconditional_branch(to))?;
            }
            Op::Pick => {
                let to = match self.start {
                    super::super::Start::Loop => i.a,
                    _ => i.b,
                };
                let to = self.edge(to, &[])?;
                b(bb.build_unconditional_branch(to))?;
            }
            Op::Brif(n_then) => self.brif(i, n_then)?,
            // only the baseline tier counts iterations
            // the first pass's function goes on in the loop's (see
            // `lower::handoff`)
            Op::TierCount { .. } => {
                let to = match self.start {
                    super::super::Start::FirstPass => i.b,
                    _ => i.c,
                };
                let to = self.edge(to, &[])?;
                b(bb.build_unconditional_branch(to))?;
            }
            Op::Call | Op::CallIndirect => self.call(i)?,
            Op::Return => {
                let v = self.v(i.a);
                b(bb.build_return(Some(&v)))?;
            }
            _ => unreachable!("not a branch, call or return"),
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
