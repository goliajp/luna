//! The control-flow half of the 5.1–5.3 encoder (see [`super::classic`]).

use super::asm::{Dist, L, Res, setlist_offset};
use super::classic::{C, FIELDS_PER_FLUSH, RK_BIT};
use super::modern::Caps;
use crate::runtime::Value;
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
            Op::Eq | Op::Lt | Op::Le | Op::EqK => {
                let k = match l.op {
                    Op::Lt => Kind::Lt,
                    Op::Le => Kind::Le,
                    _ => Kind::Eq,
                };
                let a = self.asm.r(l.a)?;
                let b = if l.op == Op::EqK {
                    self.rk(l.b)?
                } else {
                    self.asm.r(l.b)?
                };
                self.emit(self.abc(k, l.k as u32, a, b))?;
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
            Op::TForPrep => {
                let w = self.jmp(0)?;
                self.asm.jump(w, Dist::SBx, pc + 1 + l.bx as i64)?;
            }
            Op::TForCall => {
                let a = self.asm.run(l.a, 3)?;
                if self.asm.run(l.a + 4, l.c.max(1))? != a + 3 {
                    return Err(self
                        .asm
                        .err("generic-for variables outside the loop's frame"));
                }
                let w = if self.f.ver == 51 {
                    self.raw_abc(p51::OP_TFORLOOP as u32, a, 0, l.c)?
                } else {
                    self.abc(Kind::TForCall, a, 0, l.c)?
                };
                self.asm.emit(w);
            }
            // 5.2/5.3: if R(A+1) ~= nil then { R(A) := R(A+1); pc += sBx }
            // with A the control slot; 5.1's TFORLOOP did the test already
            Op::TForLoop => {
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

    /// An arithmetic operand: the constant itself (`RK`) when luna loaded
    /// it into a scratch register just before, as PUC's parser passes a
    /// constant operand. An error then names no variable for it, as in
    /// PUC; from a register filled by `LOADK` it would read "constant".
    pub(super) fn operand(&mut self, r: u32) -> Res<u32> {
        let pc = self.asm.pc();
        let local = self
            .asm
            .p
            .locvars
            .iter()
            .any(|v| v.reg == r && (v.start_pc as usize) <= pc && pc < v.end_pc as usize);
        if !local {
            for j in (pc.saturating_sub(2)..pc).rev() {
                if self.asm.is_target(j + 1) {
                    break;
                }
                let l = L::of(self.asm.inst(j).expect("an earlier pc"));
                if l.a != r {
                    if matches!(l.op, Op::LoadK | Op::LoadI | Op::LoadF) {
                        continue;
                    }
                    break;
                }
                let k = match l.op {
                    Op::LoadK => l.bx,
                    Op::LoadI => {
                        let v = self.num(l.sbx as i64);
                        self.asm.konst(v)
                    }
                    Op::LoadF => self.asm.konst(Value::Float(l.sbx as f64)),
                    _ => break,
                };
                if k < RK_BIT {
                    return Ok(k | RK_BIT);
                }
                break;
            }
        }
        self.asm.r(r)
    }

    /// luna reaches a global whose name is past constant 255 as
    /// `GetUpval t _ENV; LoadK t+1 name; GetTable r t t+1` (or `SetTable t
    /// t+1 v`); 5.1's `GETGLOBAL`/`SETGLOBAL` take the name's index whole.
    pub(super) fn global_by_register(&mut self, l: L) -> Res<usize> {
        let pc = self.asm.pc();
        let (t, key) = (l.a, l.a + 1);
        let (k, n) = match self.asm.inst(pc + 1) {
            Some(i) if i.op() == Op::LoadK && i.a() == key => (i.bx(), 2),
            Some(i) if i.op() == Op::LoadKx && i.a() == key => match self.asm.inst(pc + 2) {
                Some(x) if x.op() == Op::ExtraArg => (x.ax(), 3),
                _ => return Err(self.asm.err("LoadKx without its ExtraArg")),
            },
            _ => return Err(self.asm.err("the environment is not a value in 5.1")),
        };
        let access = self.asm.inst(pc + n).map(L::of);
        let entered = (1..=n).any(|d| self.asm.is_target(pc + d));
        let w = match access {
            Some(x) if !entered && x.op == Op::GetTable && x.b == t && x.c == key => {
                let a = self.asm.r(x.a)?;
                self.raw_abx(p51::OP_GETGLOBAL as u32, a, k)?
            }
            Some(x) if !entered && x.op == Op::SetTable && x.a == t && x.b == key => {
                let v = self.asm.r(x.c)?;
                self.raw_abx(p51::OP_SETGLOBAL as u32, v, k)?
            }
            _ => return Err(self.asm.err("the environment is not a value in 5.1")),
        };
        self.asm.emit(w);
        Ok(n + 1)
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
