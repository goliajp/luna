//! The control-flow half of the 5.4/5.5 encoder (see [`super::modern`]).

use super::asm::{Dist, L, Res, array_hint, setlist_offset};
use super::modern::{Caps, M};
use crate::vm::dump::puc::modern::Kind;
use crate::vm::isa::Op;

impl M<'_, '_> {
    /// Control flow, calls, loops, closures and varargs.
    pub(super) fn flow(&mut self, l: L, caps: &mut Caps) -> Res<usize> {
        let pc = self.asm.pc() as i64;
        match l.op {
            Op::Jmp => {
                let w = self.op(Kind::Jmp);
                self.asm.jump(w, Dist::SJ, pc + 1 + l.sj)?;
            }
            Op::Eq | Op::Lt | Op::Le => {
                let k = match l.op {
                    Op::Eq => Kind::Eq,
                    Op::Lt => Kind::Lt,
                    _ => Kind::Le,
                };
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(k, a, b, 0, l.k))?;
            }
            Op::EqK => {
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(Kind::EqK, a, l.b, 0, l.k))?;
            }
            Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI => self.cmp_i(l)?,
            Op::Test => {
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(Kind::Test, a, 0, 0, l.k))?;
            }
            Op::TestSet => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::TestSet, a, b, 0, l.k))?;
            }
            Op::Call => {
                let a = self.asm.run(l.a, l.b.max(l.c.saturating_sub(1)).max(1))?;
                self.emit(self.abc(Kind::Call, a, l.b, l.c, false))?;
            }
            Op::TailCall => {
                let a = self.asm.run(l.a, l.b.max(1))?;
                self.emit(self.abc(Kind::TailCall, a, l.b, self.f.ret_c, self.f.needclose))?;
            }
            Op::Return | Op::Return0 | Op::Return1 => self.ret(l)?,
            Op::ForPrep | Op::ForLoop => {
                let a = self.for_base(l.a)?;
                if l.op == Op::ForPrep {
                    let w = self.abx(Kind::ForPrep, a, 0)?;
                    self.asm.jump(w, Dist::BxFwd, pc + l.bx as i64)?;
                } else {
                    let w = self.abx(Kind::ForLoop, a, 0)?;
                    self.asm.jump(w, Dist::BxBack, pc + 1 - l.bx as i64)?;
                }
            }
            Op::TForPrep | Op::TForLoop => {
                let a = self.for_base(l.a)?;
                if l.op == Op::TForPrep {
                    let w = self.abx(Kind::TForPrep, a, 0)?;
                    self.asm.jump(w, Dist::BxFwd, pc + 1 + l.bx as i64)?;
                } else {
                    let w = self.abx(Kind::TForLoop, a, 0)?;
                    self.asm.jump(w, Dist::BxBack, pc + 1 - l.bx as i64)?;
                }
            }
            Op::TForCall => {
                let a = self.for_base(l.a)?;
                let first = if self.f.v55 { a + 3 } else { a + 4 };
                if self.asm.run(l.a + 4, l.c.max(1))? != first {
                    return Err(self
                        .asm
                        .err("generic-for variables outside the loop's frame"));
                }
                self.emit(self.abc(Kind::TForCall, a, 0, l.c, false))?;
            }
            Op::SetList => return self.set_list(l),
            Op::Closure => {
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
                if caps[idx].replace(cap).is_some() {
                    return Err(self
                        .asm
                        .err(format_args!("function {idx} instantiated twice")));
                }
                let a = self.asm.r(l.a)?;
                self.emit(self.abx(Kind::Closure, a, l.bx))?;
            }
            Op::Vararg => {
                let a = self.asm.run(l.a, l.c.saturating_sub(1).max(1))?;
                let w = if self.f.v55 {
                    self.abc(Kind::Vararg, a, self.f.np, l.c, self.f.vatab)
                } else {
                    self.abc(Kind::Vararg, a, 0, l.c, false)
                };
                self.emit(w)?;
            }
            Op::VargIdx if self.f.v55 => {
                let (a, c) = (self.asm.r(l.a)?, self.asm.r(l.c)?);
                let k = if self.f.vatab {
                    Kind::GetTable
                } else {
                    Kind::GetVarg
                };
                self.emit(self.abc(k, a, self.f.np, c, false))?;
            }
            Op::ErrNNil if self.f.v55 => {
                let a = self.asm.r(l.a)?;
                self.emit(self.abx(Kind::ErrNNil, a, l.bx))?;
            }
            op => return Err(self.asm.err(format_args!("{op:?} has no PUC form here"))),
        }
        Ok(1)
    }

    /// Base of a `for` loop, after checking that its hidden slots are where
    /// PUC's loop ops look for them.
    pub(super) fn for_base(&self, a: u32) -> Res<u32> {
        let base = self.asm.r(a)?;
        let (last, want) = if self.f.v55 {
            (a + 3, base + 2)
        } else {
            (a + 3, base + 3)
        };
        if self.asm.r(last)? != want || self.asm.r(a + 1)? != base + 1 {
            return Err(self.asm.err("loop slots straddle another loop's window"));
        }
        Ok(base)
    }

    pub(super) fn new_table(&mut self, l: L) -> Res<()> {
        let a = self.asm.r(l.a)?;
        let narr = array_hint(self.asm.p, self.asm.pc(), l.a, l.b);
        let hash = if l.c == 0 {
            0
        } else {
            32 - (l.c - 1).leading_zeros() + 1
        };
        let size = if self.f.v55 { 1024 } else { 256 };
        let (rc, extra) = (narr % size, narr / size);
        let w = if self.f.v55 {
            self.vabc(Kind::NewTable, a, hash, rc, extra > 0)?
        } else {
            self.abc(Kind::NewTable, a, hash, rc, extra > 0)?
        };
        self.asm.emit(w);
        self.emit(self.ax(extra as u64))
    }

    pub(super) fn set_list(&mut self, l: L) -> Res<usize> {
        let offset = setlist_offset(self.asm, l)?;
        let a = self.asm.run(l.a, l.b + 1)?;
        let size = if self.f.v55 { 1024 } else { 256 };
        let (c, extra) = ((offset % size) as u32, offset / size);
        let w = if self.f.v55 {
            self.vabc(Kind::SetList, a, l.b, c, extra > 0)?
        } else {
            self.abc(Kind::SetList, a, l.b, c, extra > 0)?
        };
        self.asm.emit(w);
        if extra > 0 {
            self.emit(self.ax(extra))?;
        }
        Ok(if l.k { 2 } else { 1 })
    }

    /// `R[A+1] := R[B]; R[A] := R[B][key]`. 5.5's `SELF` takes only a
    /// short-string constant; otherwise PUC's parser moves and indexes.
    pub(super) fn self_op(&mut self, l: L) -> Res<()> {
        let a = self.asm.run(l.a, 2)?;
        let b = self.asm.r(l.b)?;
        if l.k && (!self.f.v55 || self.asm.short_str(l.c)) {
            return self.emit(self.abc(Kind::SelfOp, a, b, l.c, true));
        }
        if !self.f.v55 {
            let c = self.asm.r(l.c)?;
            return self.emit(self.abc(Kind::SelfOp, a, b, c, false));
        }
        let mut key = if l.k {
            let t = self.asm.temp()?;
            self.load_k(t, l.c)?;
            t
        } else {
            self.asm.r(l.c)?
        };
        if key == a + 1 {
            let t = self.asm.temp()?;
            self.emit(self.abc(Kind::Move, t, key, 0, false))?;
            key = t;
        }
        if b != a + 1 {
            self.emit(self.abc(Kind::Move, a + 1, b, 0, false))?;
        }
        self.emit(self.abc(Kind::GetTable, a, b, key, false))
    }

    pub(super) fn ret(&mut self, l: L) -> Res<()> {
        let (a, b) = match l.op {
            Op::Return0 => (0, 1),
            Op::Return1 => (self.asm.r(l.a)?, 2),
            _ => (self.asm.run(l.a, l.b.saturating_sub(1).max(1))?, l.b),
        };
        let plain = !self.f.needclose && self.f.ret_c == 0;
        let w = match b {
            1 if plain => self.abc(Kind::Return0, a, 1, 0, false)?,
            2 if plain => self.abc(Kind::Return1, a, 2, 0, false)?,
            _ => self.abc(Kind::Return, a, b, self.f.ret_c, self.f.needclose)?,
        };
        self.asm.emit(w);
        Ok(())
    }
}
