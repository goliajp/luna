//! The control-flow half of the 5.1–5.3 encoder (see [`super::classic`]).

use super::asm::{Dist, L, Res, setlist_offset};
use super::classic::{C, FIELDS_PER_FLUSH};
use super::modern::Caps;
use crate::vm::dump::puc::classic::Kind;
use crate::vm::dump::puc::puc_51 as p51;
use crate::vm::isa::Op;

impl C<'_, '_> {
    /// Control flow, calls, loops, closures and varargs.
    pub(super) fn flow(&mut self, l: L, caps: &mut Caps) -> Res<usize> {
        let pc = self.asm.pc() as i64;
        match l.op {
            Op::Jmp => {
                let w = self.jmp(0)?;
                self.asm.jump(w, Dist::SBx, pc + 1 + l.sj)?;
            }
            Op::JmpClose | Op::JmpCloseBack if self.f.ver != 51 => {
                let a = self.asm.r(l.a - 1)?;
                let w = self.jmp(a + 1)?;
                self.asm.jump(w, Dist::SBx, pc + 1 + l.sj)?;
            }
            Op::Eq | Op::Lt | Op::Le => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(cmp_kind(l.op), l.k as u32, a, b))?;
            }
            // `C`: the constant was the left operand
            Op::EqK | Op::LtK | Op::LeK => {
                let (a, b) = (self.asm.r(l.a)?, self.rk(l.b)?);
                let (x, y) = if l.c != 0 { (b, a) } else { (a, b) };
                self.emit(self.abc(cmp_kind(l.op), l.k as u32, x, y))?;
            }
            Op::EqKK | Op::LtKK | Op::LeKK => {
                let (a, b) = (self.rk(l.a)?, self.rk(l.b)?);
                self.emit(self.abc(cmp_kind(l.op), l.k as u32, a, b))?;
            }
            Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI => self.cmp_const(l)?,
            Op::Test => {
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(Kind::Test, a, 0, l.k as u32))?;
            }
            Op::TestSet => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::TestSet, a, b, l.k as u32))?;
            }
            Op::Call => {
                let a = self.asm.run(l.a, l.b.max(l.c.saturating_sub(1)).max(1))?;
                self.emit(self.abc(Kind::Call, a, l.b, l.c))?;
            }
            Op::TailCall => {
                let a = self.asm.run(l.a, l.b.max(1))?;
                self.emit(self.abc(Kind::TailCall, a, l.b, 0))?;
            }
            Op::Return0 => self.emit(self.abc(Kind::Return, 0, 1, 0))?,
            Op::Return1 => {
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(Kind::Return, a, 2, 0))?;
            }
            Op::Return => {
                let a = self.asm.run(l.a, l.b.saturating_sub(1).max(1))?;
                self.emit(self.abc(Kind::Return, a, l.b, 0))?;
            }
            // FORPREP jumps to its FORLOOP, which jumps back to the body
            Op::ForPrep | Op::ForLoop => {
                let a = self.asm.run(l.a, 4)?;
                let (k, target) = if l.op == Op::ForPrep {
                    (Kind::ForPrep, pc + l.bx as i64)
                } else {
                    (Kind::ForLoop, pc + 1 - l.bx as i64)
                };
                let w = self.abx(k, a, 0)?;
                self.asm.jump(w, Dist::SBx, target)?;
            }
            Op::TForPrep53 => {
                let w = self.jmp(0)?;
                self.asm.jump(w, Dist::SBx, pc + 1 + l.bx as i64)?;
            }
            Op::TForCall53 => {
                let a = self.asm.run(l.a, 3 + l.c.max(1))?;
                let w = if self.f.ver == 51 {
                    self.raw_abc(p51::OP_TFORLOOP as u32, a, 0, l.c)?
                } else {
                    self.abc(Kind::TForCall, a, 0, l.c)?
                };
                self.asm.emit(w);
            }
            // 5.2/5.3: if R(A+1) ~= nil then { R(A) := R(A+1); pc += sBx }
            // with A the control slot; 5.1's TFORLOOP did the test already
            Op::TForLoop53 => {
                let a = self.asm.r(l.a)?;
                let w = if self.f.ver == 51 {
                    self.jmp(0)?
                } else {
                    self.raw_abx(self.op(Kind::TForLoop)?, a + 2, 0)?
                };
                self.asm.jump(w, Dist::SBx, pc + 1 - l.bx as i64)?;
            }
            Op::SetList => return self.set_list(l),
            Op::Closure => self.closure(l, caps)?,
            Op::Vararg => {
                let a = self.asm.run(l.a, l.c.saturating_sub(1).max(1))?;
                self.emit(self.abc(Kind::Vararg, a, l.c, 0))?;
            }
            op => {
                return Err(self
                    .asm
                    .err(format_args!("{op:?} has no form in this dialect")));
            }
        }
        Ok(1)
    }

    /// 5.1 globals live in the function environment, luna's upvalue 0.
    pub(super) fn global(&self, up: u32) -> Res<()> {
        if up != 0 {
            return Err(self
                .asm
                .err("table access through a non-environment upvalue"));
        }
        Ok(())
    }

    pub(super) fn set_list(&mut self, l: L) -> Res<usize> {
        let offset = setlist_offset(self.asm, l)?;
        if offset % FIELDS_PER_FLUSH != 0 {
            return Err(self.asm.err("table constructor flush off a 50-field block"));
        }
        let block = offset / FIELDS_PER_FLUSH + 1;
        let a = self.asm.run(l.a, l.b + 1)?;
        if block <= 511 {
            self.emit(self.abc(Kind::SetList, a, l.b, block as u32))?;
        } else if block < 1 << 26 {
            self.emit(self.abc(Kind::SetList, a, l.b, 0))?;
            // 5.1 stores the block number as a raw code word
            let extra = if self.f.ver == 51 {
                0
            } else {
                self.op(Kind::ExtraArg)?
            };
            self.emit(Ok(
                extra | ((block as u32) << if self.f.ver == 51 { 0 } else { 6 })
            ))?;
        } else {
            return Err(self.asm.err("table constructor too long"));
        }
        Ok(if l.k { 2 } else { 1 })
    }

    pub(super) fn closure(&mut self, l: L, caps: &mut Caps) -> Res<()> {
        let idx = l.bx as usize;
        let Some(child) = self.asm.p.protos.get(idx) else {
            return Err(self
                .asm
                .err(format_args!("closure of missing function {idx}")));
        };
        let mut cap = Vec::with_capacity(child.upvals.len());
        for u in child.upvals.iter() {
            let index = if u.in_stack {
                self.asm.r(u.index as u32)? as u8
            } else {
                u.index
            };
            cap.push((u.in_stack, index));
        }
        if self.f.ver == 51
            && !matches!(child.upvals.first(), Some(u) if &*u.name == "_ENV" && !u.in_stack && u.index == 0)
        {
            return Err(self
                .asm
                .err("function without the environment as upvalue 0"));
        }
        let a = self.asm.r(l.a)?;
        self.emit(self.abx(Kind::Closure, a, l.bx))?;
        if self.f.ver == 51 {
            for &(in_stack, index) in &cap[1..] {
                let w = if in_stack {
                    self.abc(Kind::Move, 0, index as u32, 0)?
                } else {
                    self.abc(Kind::GetUpval, 0, self.upval(index as u32)?, 0)?
                };
                self.asm.emit(w);
            }
        }
        if caps[idx].replace(cap).is_some() {
            return Err(self
                .asm
                .err(format_args!("function {idx} instantiated twice")));
        }
        Ok(())
    }
}

/// The comparison an `EQ` / `LT` / `LE` writes.
fn cmp_kind(op: Op) -> Kind {
    match op {
        Op::Lt | Op::LtK | Op::LtKK => Kind::Lt,
        Op::Le | Op::LeK | Op::LeKK => Kind::Le,
        _ => Kind::Eq,
    }
}
