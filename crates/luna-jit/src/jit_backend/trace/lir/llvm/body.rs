//! The function's code: a block making the variables' and stack slots'
//! homes, then the trace's blocks in layout order.

use super::*;

impl<'c> Gen<'c, '_> {
    /// `root`: the block the function starts at; `e`, in the loop's
    /// function, where it starts, and what it reads first: per variable,
    /// whether it is one of the registers `e.regs` names read from
    /// `reg_state`, and the values the loop head takes from the variables
    /// (see `clif::live_in_vars`).
    pub(super) fn body(
        &mut self,
        an: &live::Analysis,
        root: u32,
        e: Option<&LoopEntry>,
        (reads, taken): (&[bool], &[(u32, u32)]),
    ) -> R<()> {
        let lir = self.lir;
        let entry = self.ctx.append_basic_block(self.f, "homes");
        for &blk in &an.order {
            self.blocks[blk as usize] = Some(self.ctx.append_basic_block(self.f, ""));
        }
        self.b.position_at_end(entry);
        self.homes()?;
        if let Some(e) = e {
            self.read_entry(an, e, reads, taken)?;
        }
        let first = self.blocks[root as usize].ok_or("llvm:no-entry-block")?;
        b(self.b.build_unconditional_branch(first))?;
        self.block_params(an, root)?;
        for &blk in &an.order {
            let bb = self.blocks[blk as usize].expect("laid out");
            self.b.position_at_end(bb);
            let (lo, hi) = an.block_at[blk as usize];
            for c in lo..hi {
                let i = lir.insts[an.code[c as usize] as usize];
                if e.is_some() {
                    self.inherited_uses(&i, blk)?;
                }
                self.inst(&i)?;
                if e.is_some() {
                    self.inherited_def(&i, blk);
                }
            }
        }
        Ok(())
    }

    /// The variables' and stack slots' homes, and the function's parameter.
    fn homes(&mut self) -> R<()> {
        let lir = self.lir;
        // a variable read on a path that never wrote it is zero, as in
        // Cranelift
        for &t in &lir.var_ty {
            let home = self.zeroed_home(t)?;
            self.vars.push(home);
        }
        for &(size, align) in &lir.slots {
            let ty = self.ctx.i8_type().array_type(size);
            let slot = b(self.b.build_alloca(ty, ""))?;
            let inst = slot.as_instruction().ok_or("llvm:builder")?;
            inst.set_alignment(1 << align).map_err(|_| "llvm:builder")?;
            self.slots.push(slot);
        }
        if lir.arg0 != NONE {
            let p = self.f.get_nth_param(0).ok_or("llvm:builder")?;
            self.vals[lir.arg0 as usize] = Some(p);
        }
        Ok(())
    }

    /// In the loop's function: notes the values the loop head takes from
    /// the variables, and reads the registers `reads` marks from
    /// `reg_state` into their variables.
    fn read_entry(
        &mut self,
        an: &live::Analysis,
        e: &LoopEntry,
        reads: &[bool],
        taken: &[(u32, u32)],
    ) -> R<()> {
        let lir = self.lir;
        self.inherit = vec![None; lir.value_ty.len()];
        for &(k, v) in taken {
            self.inherit[v as usize] = Some((k, NONE, None));
        }
        self.idom = super::super::clif::idoms(lir, an);
        for (i, &var) in e.regs.iter().enumerate() {
            if reads.get(var as usize).copied().unwrap_or(false) {
                let p = self.addr(self.iv(lir.arg0), (i as i32) * 8)?;
                let x = self.load(self.ctx.i64_type().into(), p)?;
                b(self.b.build_store(self.vars[var as usize], x))?;
            }
        }
        Ok(())
    }

    /// A phi for each parameter of each block but `root`.
    fn block_params(&mut self, an: &live::Analysis, root: u32) -> R<()> {
        let lir = self.lir;
        for &blk in an.order.iter().filter(|&&blk| blk != root) {
            let bb = self.blocks[blk as usize].expect("laid out");
            self.b.position_at_end(bb);
            for &p in lir.block_params(blk) {
                let phi = b(self.b.build_phi(self.ty(lir.value_ty[p as usize]), ""))?;
                self.vals[p as usize] = Some(phi.as_basic_value());
                self.phis[blk as usize].push(phi);
            }
        }
        Ok(())
    }

    /// A stack home of type `t`, zeroed.
    fn zeroed_home(&mut self, t: Ty) -> R<PointerValue<'c>> {
        let home = b(self.b.build_alloca(self.ty(t), ""))?;
        let zero: BasicValueEnum = match t {
            Ty::F64 => self.ctx.f64_type().const_zero().into(),
            _ => self.ity(t).const_zero().into(),
        };
        b(self.b.build_store(home, zero))?;
        Ok(home)
    }

    /// Reads each inherited value `i` (in block `blk`) reads from the
    /// variable it was taken from where its definition does not dominate
    /// `blk` (see `clif::Replay::inherited_uses`).
    fn inherited_uses(&mut self, i: &Inst, blk: u32) -> R<()> {
        let nv = self.lir.value_ty.len() as u32;
        let mut uses = Vec::new();
        live::for_uses(self.lir, i, |x| {
            if x < nv && self.inherit[x as usize].is_some() {
                uses.push(x);
            }
        });
        for x in uses {
            let Some((k, at, def)) = self.inherit[x as usize] else {
                continue;
            };
            if at == NONE || !super::super::clif::dominates(&self.idom, at, blk) {
                let ty = self.ty(self.lir.var_ty[k as usize]);
                let v = b(self.b.build_load(ty, self.vars[k as usize], ""))?;
                self.vals[x as usize] = Some(v);
            } else if let Some(d) = def {
                self.vals[x as usize] = Some(d);
            }
        }
        Ok(())
    }

    /// Notes where an inherited value `i` defines was defined.
    fn inherited_def(&mut self, i: &Inst, blk: u32) {
        if i.dst != NONE
            && let Some(Some((_, at, def))) = self.inherit.get_mut(i.dst as usize)
        {
            *at = blk;
            *def = self.vals[i.dst as usize];
        }
    }
}
