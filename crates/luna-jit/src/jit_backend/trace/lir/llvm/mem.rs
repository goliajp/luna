//! Loads, stores and stack slots as LLVM IR.

use super::*;

impl<'c> Gen<'c, '_> {
    pub(super) fn load(&self, ty: BasicTypeEnum<'c>, p: PointerValue<'c>) -> R<BasicValueEnum<'c>> {
        let v = b(self.b.build_load(ty, p, ""))?;
        if let Some(inst) = v.as_instruction_value() {
            inst.set_alignment(1).map_err(|_| "llvm:builder")?;
        }
        Ok(v)
    }

    pub(super) fn store(&self, v: BasicValueEnum<'c>, p: PointerValue<'c>) -> R<()> {
        let inst = b(self.b.build_store(p, v))?;
        inst.set_alignment(1).map_err(|_| "llvm:builder")
    }

    /// A memory access, stack slot access or variable access.
    pub(super) fn mem(&mut self, i: &Inst) -> R<()> {
        let bb = self.b;
        match i.op {
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
            Op::VarRead => {
                let home = self.vars[i.a as usize];
                let v = b(bb.build_load(self.ty(self.lir.var_ty[i.a as usize]), home, ""))?;
                self.set(i.dst, v);
            }
            Op::VarWrite => {
                b(bb.build_store(self.vars[i.a as usize], self.v(i.b)))?;
            }
            _ => unreachable!("not a memory access"),
        }
        Ok(())
    }
}
