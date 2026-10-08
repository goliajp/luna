//! luna instructions → PUC 5.1 / 5.2 / 5.3 instructions.
//!
//! The older layout is `op:6 | A:8 | C:9 | B:9`, with 9-bit `RK` operands
//! that name a constant (index up to 255) when their top bit is set. What
//! the encoding has to add to luna's code:
//!
//! - luna's immediates (`LoadI`, `LoadF`, `GetI`, `SetI`) and constant
//!   field keys become constants, as a number of the dialect (5.1 and 5.2
//!   have no integers); the constant- and immediate-operand arithmetic
//!   and comparisons take theirs as `RK` operands ([`super::classic_const`]);
//! - luna's `Close` is `OP_CLOSE` in 5.1 and a `JMP` that closes in
//!   5.2/5.3;
//! - the generic `for` is entered by a `JMP` to its call, and its loop test
//!   names the control register (`A + 2`); in 5.1 the call and the loop
//!   test are one `TFORLOOP` followed by the back `JMP`;
//! - 5.1 reads globals with `GETGLOBAL`/`SETGLOBAL` from the function
//!   environment, which luna keeps as upvalue 0 (`_ENV`): that upvalue is
//!   dropped, the others move down one, and a `CLOSURE` is followed by one
//!   pseudo-instruction per captured variable;
//! - `SETLIST` counts blocks of 50 fields from 1.

use super::asm::{Asm, L, Res};
use super::classic_ops::op51;
use super::modern::Caps;
use crate::runtime::Value;
use crate::vm::dump::puc::classic::Kind;
use crate::vm::dump::puc::puc_51 as p51;
use crate::vm::isa::Op;

pub(super) struct Frame {
    /// 51, 52 or 53
    pub ver: u8,
    /// 5.2 / 5.3 opcode numbering
    pub ops: &'static [Kind],
}

/// `LFIELDS_PER_FLUSH` (lopcodes.h, all three versions).
pub(super) const FIELDS_PER_FLUSH: u64 = 50;
pub(super) const RK_BIT: u32 = 256;
const MAX_BX: u32 = (1 << 18) - 1;

pub(super) struct C<'a, 'p> {
    pub asm: &'a mut Asm<'p>,
    pub f: &'a Frame,
}

impl C<'_, '_> {
    pub(super) fn op(&self, k: Kind) -> Res<u32> {
        let n = if self.f.ver == 51 {
            op51(k).map(u32::from)
        } else {
            self.f.ops.iter().position(|&x| x == k).map(|n| n as u32)
        };
        n.ok_or_else(|| self.asm.err("instruction has no form in this dialect"))
    }

    pub(super) fn abc(&self, k: Kind, a: u32, b: u32, c: u32) -> Res<u32> {
        let op = self.op(k)?;
        self.raw_abc(op, a, b, c)
    }

    pub(super) fn raw_abc(&self, op: u32, a: u32, b: u32, c: u32) -> Res<u32> {
        if a > 255 || b > 511 || c > 511 {
            return Err(self
                .asm
                .err(format_args!("operands {a} {b} {c} do not fit")));
        }
        Ok(op | (a << 6) | (c << 14) | (b << 23))
    }

    pub(super) fn raw_abx(&self, op: u32, a: u32, bx: u32) -> Res<u32> {
        if a > 255 || bx > MAX_BX {
            return Err(self.asm.err(format_args!("operands {a} {bx} do not fit")));
        }
        Ok(op | (a << 6) | (bx << 14))
    }

    pub(super) fn abx(&self, k: Kind, a: u32, bx: u32) -> Res<u32> {
        let op = self.op(k)?;
        self.raw_abx(op, a, bx)
    }

    pub(super) fn emit(&mut self, w: Res<u32>) -> Res<()> {
        self.asm.emit(w?);
        Ok(())
    }

    /// The dialect's number for a luna integer immediate.
    pub(super) fn num(&self, i: i64) -> Value {
        if self.f.ver >= 53 {
            Value::Int(i)
        } else {
            Value::Float(i as f64)
        }
    }

    pub(super) fn load_k(&mut self, dst: u32, k: u32) -> Res<()> {
        if k <= MAX_BX {
            return self.emit(self.abx(Kind::LoadK, dst, k));
        }
        if self.f.ver == 51 || k >= 1 << 26 {
            return Err(self.asm.err(format_args!("constant {k} out of reach")));
        }
        self.emit(self.abx(Kind::LoadKx, dst, 0))?;
        let x = self.op(Kind::ExtraArg)?;
        self.emit(Ok(x | (k << 6)))
    }

    /// Luna upvalue `u` in 5.1, which has no `_ENV` upvalue.
    pub(super) fn upval(&self, u: u32) -> Res<u32> {
        if self.f.ver != 51 {
            return Ok(u);
        }
        u.checked_sub(1)
            .ok_or_else(|| self.asm.err("the environment is not a value in 5.1"))
    }

    pub(super) fn jmp(&self, close: u32) -> Res<u32> {
        let op = self.op(Kind::Jmp)?;
        self.raw_abc(op, close, 0, 0)
    }

    /// Returns how many luna instructions it consumed.
    pub(super) fn one(&mut self, l: L, caps: &mut Caps) -> Res<usize> {
        match l.op {
            Op::Move => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::Move, a, b, 0))?;
            }
            Op::LoadI | Op::LoadF => {
                let v = if l.op == Op::LoadI {
                    self.num(l.sbx as i64)
                } else {
                    Value::Float(l.sbx as f64)
                };
                let (a, k) = (self.asm.r(l.a)?, self.asm.konst(v));
                self.load_k(a, k)?;
            }
            Op::LoadK => {
                let a = self.asm.r(l.a)?;
                self.load_k(a, l.bx)?;
            }
            Op::LoadKx => {
                let a = self.asm.r(l.a)?;
                let Some(x) = self
                    .asm
                    .inst(self.asm.pc() + 1)
                    .filter(|x| x.op() == Op::ExtraArg)
                else {
                    return Err(self.asm.err("LoadKx without its ExtraArg"));
                };
                self.load_k(a, x.ax())?;
                return Ok(2);
            }
            Op::LoadFalse | Op::LFalseSkip | Op::LoadTrue => {
                let a = self.asm.r(l.a)?;
                let (b, c) = match l.op {
                    Op::LoadFalse => (0, 0),
                    Op::LFalseSkip => (0, 1),
                    _ => (1, 0),
                };
                self.emit(self.abc(Kind::LoadBool, a, b, c))?;
            }
            Op::LoadNil => {
                let a = self.asm.run(l.a, l.b + 1)?;
                let b = if self.f.ver == 51 { a + l.b } else { l.b };
                self.emit(self.abc(Kind::LoadNil, a, b, 0))?;
            }
            Op::GetUpval | Op::SetUpval => {
                let k = if l.op == Op::GetUpval {
                    Kind::GetUpval
                } else {
                    Kind::SetUpval
                };
                let (a, b) = (self.asm.r(l.a)?, self.upval(l.b)?);
                self.emit(self.abc(k, a, b, 0))?;
            }
            Op::GetTabUp => {
                let a = self.asm.r(l.a)?;
                if self.f.ver == 51 {
                    self.global(l.b)?;
                    self.emit(self.raw_abx(p51::OP_GETGLOBAL as u32, a, l.c))?;
                } else {
                    self.emit(self.abc(Kind::GetTabUp, a, l.b, l.c | RK_BIT))?;
                }
            }
            Op::SetTabUp => {
                let c = self.store_val(l)?;
                if self.f.ver == 51 {
                    self.global(l.a)?;
                    self.emit(self.raw_abx(p51::OP_SETGLOBAL as u32, c, l.b))?;
                } else {
                    self.emit(self.abc(Kind::SetTabUp, l.a, l.b | RK_BIT, c))?;
                }
            }
            Op::GetTabUpR | Op::SetTabUpR | Op::SetTabUpK => self.tab_up_rk(l)?,
            Op::GetGlobal | Op::SetGlobal if self.f.ver == 51 => {
                let op = if l.op == Op::GetGlobal {
                    p51::OP_GETGLOBAL
                } else {
                    p51::OP_SETGLOBAL
                };
                let a = self.asm.r(l.a)?;
                self.emit(self.raw_abx(op as u32, a, l.bx))?;
            }
            Op::GetTable | Op::GetI | Op::GetField | Op::GetTableK => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                let key = match l.op {
                    Op::GetTable => self.asm.r(l.c)?,
                    Op::GetI => self.rk_num(l.c as i64)?,
                    _ => self.rk(l.c)?,
                };
                self.emit(self.abc(Kind::GetTable, a, b, key))?;
            }
            Op::SetTable | Op::SetI | Op::SetField | Op::SetTableK => {
                let a = self.asm.r(l.a)?;
                let key = match l.op {
                    Op::SetTable => self.asm.r(l.b)?,
                    Op::SetI => self.rk_num(l.b as i64)?,
                    _ => self.rk(l.b)?,
                };
                let c = self.store_val(l)?;
                self.emit(self.abc(Kind::SetTable, a, key, c))?;
            }
            Op::NewTable => {
                // a NewTable of a 5.4 / 5.5 chunk holds its sizes plainly
                let a = self.asm.r(l.a)?;
                let (b, c) = if l.k {
                    (l.b, l.c)
                } else {
                    let (asize, hsize) = crate::runtime::table::new_table_sizes(l.b, l.c, false)
                        .ok_or_else(|| self.asm.err("NewTable sizes past a table's limit"))?;
                    use crate::runtime::table::int2fb;
                    (int2fb(asize as u32), int2fb(hsize as u32))
                };
                self.emit(self.abc(Kind::NewTable, a, b, c))?;
            }
            Op::SelfOp => {
                let (a, b) = (self.asm.run(l.a, 2)?, self.asm.r(l.b)?);
                let key = if l.k { self.rk(l.c)? } else { self.asm.r(l.c)? };
                self.emit(self.abc(Kind::SelfOp, a, b, key))?;
            }
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Mod
            | Op::Pow
            | Op::Div
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => {
                let (a, b, c) = (self.asm.r(l.a)?, self.asm.r(l.b)?, self.asm.r(l.c)?);
                self.emit(self.abc(Kind::Arith(l.op), a, b, c))?;
            }
            op if op.arith_const_op().is_some() => self.arith_const(l)?,
            op if op.arith_kk_op().is_some() => {
                let kind = Kind::Arith(op.arith_kk_op().expect("checked"));
                let (a, b, c) = (self.asm.r(l.a)?, self.rk(l.b)?, self.rk(l.c)?);
                self.emit(self.abc(kind, a, b, c))?;
            }
            Op::Unm | Op::BNot | Op::Not | Op::Len => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::Unary(l.op), a, b, 0))?;
            }
            Op::Concat => {
                if l.b < 2 {
                    return Err(self.asm.err("concatenation of fewer than two values"));
                }
                let (first, out) = if l.k { (l.c, l.a) } else { (l.a, l.a) };
                let (a, first) = (self.asm.r(out)?, self.asm.run(first, l.b)?);
                self.emit(self.abc(Kind::Concat, a, first, first + l.b - 1))?;
            }
            Op::Close => {
                let a = self.asm.r(l.a)?;
                let w = if self.f.ver == 51 {
                    self.raw_abc(p51::OP_CLOSE as u32, a, 0, 0)?
                } else {
                    // `JMP A+1 0`: close upvalues from R(A), fall through
                    self.jmp(a + 1)? | ((1 << 17) - 1) << 14
                };
                self.asm.emit(w);
            }
            _ => return self.flow(l, caps),
        }
        Ok(1)
    }
}

pub(super) fn encode(asm: &mut Asm, f: &Frame, caps: &mut Caps) -> Res<()> {
    let p = asm.p;
    let mut pc = 0;
    while pc < p.code.len() {
        asm.begin(pc);
        let l = L::of(p.code[pc]);
        pc += C { asm, f }.one(l, caps)?;
    }
    Ok(())
}
