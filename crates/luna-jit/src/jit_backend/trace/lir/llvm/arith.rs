//! Float arithmetic with Cranelift's NaN signs, and the unary operations.

use super::*;
use inkwell::values::BasicValueEnum;

impl<'c> Gen<'c, '_> {
    /// `x op y` as the machine computes it. Plain LLVM float operations
    /// fold on constants to a NaN of LLVM's choosing (positive), where
    /// x86 makes a negative one, and Lua prints the sign. LLVM folds a
    /// constrained operation that raises invalid only when exceptions are
    /// not strict.
    pub(super) fn float_bin(
        &self,
        op: BinOp,
        x: FloatValue<'c>,
        y: FloatValue<'c>,
    ) -> R<BasicValueEnum<'c>> {
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

    pub(super) fn un(&self, u: UnOp, i: &Inst) -> R<BasicValueEnum<'c>> {
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
}
