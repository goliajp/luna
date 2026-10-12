//! The control-flow half of the 5.4/5.5 encoder (see [`super::modern`]).

use super::asm::{Dist, L, Res, setlist_offset};
use super::modern::{Caps, M};
use crate::vm::dump::puc::modern::Kind;
use crate::vm::isa::{ForLayout, Op};

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
            op if op.is_for_prep()
                || op.is_for_loop()
                || op.is_tfor_prep()
                || op.is_tfor_loop() =>
            {
                let a = self.for_base(l)?;
                let (prep, back) = if op.is_for_prep() || op.is_for_loop() {
                    (Kind::ForPrep, Kind::ForLoop)
                } else {
                    (Kind::TForPrep, Kind::TForLoop)
                };
                if op.is_for_prep() {
                    let w = self.abx(prep, a, 0)?;
                    self.asm.jump(w, Dist::BxFwd, pc + l.bx as i64)?;
                } else if op.is_tfor_prep() {
                    let w = self.abx(prep, a, 0)?;
                    self.asm.jump(w, Dist::BxFwd, pc + 1 + l.bx as i64)?;
                } else {
                    let w = self.abx(back, a, 0)?;
                    self.asm.jump(w, Dist::BxBack, pc + 1 - l.bx as i64)?;
                }
            }
            op if op.is_tfor_call() => {
                let a = self.for_base(l)?;
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

    /// Base of a `for` loop, whose layout must be the dialect's own: the
    /// loop of another dialect's chunk has no form here.
    pub(super) fn for_base(&self, l: L) -> Res<u32> {
        let ok = match l.op.for_layout() {
            Some(ForLayout::Num) | Some(ForLayout::Gen54) => !self.f.v55,
            Some(ForLayout::Num55) | Some(ForLayout::Gen55) => self.f.v55,
            _ => false,
        };
        if !ok {
            return Err(self.asm.err("a loop of another dialect's layout"));
        }
        let lay = l.op.for_layout().expect("checked above");
        self.asm.run(l.a, lay.var() + 1)
    }

    pub(super) fn new_table(&mut self, l: L) -> Res<()> {
        let a = self.asm.r(l.a)?;
        // a NewTable of a 5.1–5.3 chunk holds its sizes the classic way
        let (narr, hash) = if l.k {
            let (asize, hsize) = crate::runtime::table::new_table_sizes(l.b, l.c, true)
                .ok_or_else(|| self.asm.err("NewTable sizes past a table's limit"))?;
            let code = if hsize == 0 {
                0
            } else {
                hsize.next_power_of_two().trailing_zeros() + 1
            };
            (asize.min(0xFF) as u32, code)
        } else {
            (self.ctor_array_size(l), l.b)
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

    /// The number of positional items of the constructor the `NewTable`
    /// `l` starts. luna's hint stops at 255; past it the count is read off
    /// the constructor's last `SetList` (PUC `luaK_settablesize`).
    fn ctor_array_size(&self, l: L) -> u32 {
        if l.c < 0xFF {
            return l.c;
        }
        let mut last = None;
        let mut pc = self.asm.pc() + 1;
        while let Some(i) = self.asm.inst(pc) {
            match i.op() {
                Op::NewTable if i.a() == l.a => break,
                Op::SetList if i.a() == l.a => last = Some(pc),
                _ => {}
            }
            pc += 1;
        }
        let Some(pc) = last else {
            return l.c;
        };
        let i = self.asm.inst(pc).expect("a SetList seen above");
        let offset = if i.k() {
            self.asm.inst(pc + 1).map_or(0, |x| x.ax())
        } else {
            i.c()
        };
        if i.b() > 0 {
            return offset + i.b();
        }
        // an open last item: the call or `...` before the SetList sits right
        // after the fixed items of its batch
        match self.asm.inst(pc - 1) {
            Some(x) if matches!(x.op(), Op::Call | Op::Vararg) => offset + x.a() - l.a - 1,
            _ => offset,
        }
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
            Op::Return0 => (self.asm.r(l.a)?, 1),
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
