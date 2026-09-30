//! luna instructions → PUC 5.4 / 5.5 instructions.
//!
//! luna's ISA has 5.4's layout, so most instructions keep their operands.
//! What changes, mirroring what `lvm.c` of each version expects:
//!
//! - arithmetic is followed by the `MMBIN` naming its metamethod event;
//!   luna's flagged `Add` (a source `x - 0`) is PUC's `ADDI x 0` with a
//!   `__sub` `MMBINI`; the constant- and immediate-operand forms are in
//!   [`super::modern_const`];
//! - the fast field ops (`GETFIELD`, `GETTABUP`, `SETFIELD`, `SETTABUP`,
//!   and `SELF` in 5.5) take short-string keys only, so a longer key goes
//!   through a register as PUC's parser does;
//! - `NEWTABLE` always has its `EXTRAARG`, and carries the size hints
//!   PUC's parser would give the constructor;
//! - a vararg function starts with `VARARGPREP`, and its returns and tail
//!   calls tell PUC where the frame really starts (`C`) and whether it has
//!   upvalues or to-be-closed variables to close (`k`);
//! - 5.5 keeps two hidden slots in a numeric `for` and three in a generic
//!   one where luna keeps three and four, so each loop is a register window.

use super::asm::{Asm, L, Res};
use crate::vm::dump::puc::modern::Kind;
use crate::vm::isa::Op;

/// What a function's encoding depends on besides its code.
pub(super) struct Frame {
    pub ops: &'static [Kind],
    pub v55: bool,
    pub np: u32,
    /// `C` of `RETURN` / `TAILCALL`: `numparams + 1` when the frame was
    /// moved above hidden varargs, otherwise 0
    pub ret_c: u32,
    /// `k` of `RETURN` / `TAILCALL`: some local is captured or to be closed
    pub needclose: bool,
    /// 5.5 `PF_VATAB`: the vararg parameter is a real table
    pub vatab: bool,
}

/// A child's upvalue descriptors as its `Closure` site maps them.
pub(super) type Caps = Vec<Option<Vec<(bool, u8)>>>;

fn opn(ops: &[Kind], k: Kind) -> u32 {
    ops.iter()
        .position(|&x| x == k)
        .expect("every emitted kind has an opcode") as u32
}

/// `ltm.h` `TMS` of an arithmetic operator (the same in 5.4 and 5.5).
pub(super) fn event(op: Op) -> Option<u32> {
    Some(match op {
        Op::Add => 6,
        Op::Sub => 7,
        Op::Mul => 8,
        Op::Mod => 9,
        Op::Pow => 10,
        Op::Div => 11,
        Op::IDiv => 12,
        Op::BAnd => 13,
        Op::BOr => 14,
        Op::BXor => 15,
        Op::Shl => 16,
        Op::Shr => 17,
        _ => return None,
    })
}

pub(super) struct M<'a, 'p> {
    pub asm: &'a mut Asm<'p>,
    pub f: &'a Frame,
}

impl M<'_, '_> {
    pub(super) fn op(&self, k: Kind) -> u32 {
        opn(self.f.ops, k)
    }

    pub(super) fn abc(&self, k: Kind, a: u32, b: u32, c: u32, kf: bool) -> Res<u32> {
        self.raw_abc(self.op(k), a, b, c, kf)
    }

    pub(super) fn raw_abc(&self, op: u32, a: u32, b: u32, c: u32, kf: bool) -> Res<u32> {
        if a > 255 || b > 255 || c > 255 {
            return Err(self
                .asm
                .err(format_args!("operands {a} {b} {c} do not fit")));
        }
        Ok(op | (a << 7) | ((kf as u32) << 15) | (b << 16) | (c << 24))
    }

    /// 5.5's `NEWTABLE` / `SETLIST` layout: 6-bit `vB`, 10-bit `vC`.
    pub(super) fn vabc(&self, k: Kind, a: u32, vb: u32, vc: u32, kf: bool) -> Res<u32> {
        if a > 255 || vb > 63 || vc > 1023 {
            return Err(self
                .asm
                .err(format_args!("operands {a} {vb} {vc} do not fit")));
        }
        Ok(self.op(k) | (a << 7) | ((kf as u32) << 15) | (vb << 16) | (vc << 22))
    }

    pub(super) fn abx(&self, k: Kind, a: u32, bx: u32) -> Res<u32> {
        if a > 255 || bx >= 1 << 17 {
            return Err(self.asm.err(format_args!("operands {a} {bx} do not fit")));
        }
        Ok(self.op(k) | (a << 7) | (bx << 15))
    }

    pub(super) fn asbx(&self, k: Kind, a: u32, sbx: i32) -> Res<u32> {
        self.abx(k, a, (sbx + 65535) as u32)
    }

    pub(super) fn ax(&self, ax: u64) -> Res<u32> {
        if ax >= 1 << 25 {
            return Err(self.asm.err("extra argument does not fit"));
        }
        Ok(self.op(Kind::ExtraArg) | ((ax as u32) << 7))
    }

    pub(super) fn emit(&mut self, w: Res<u32>) -> Res<()> {
        self.asm.emit(w?);
        Ok(())
    }

    pub(super) fn load_k(&mut self, dst: u32, k: u32) -> Res<()> {
        self.emit(self.abx(Kind::LoadK, dst, k))
    }

    /// Returns how many luna instructions it consumed.
    pub(super) fn one(&mut self, l: L, caps: &mut Caps) -> Res<usize> {
        match l.op {
            Op::Move => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::Move, a, b, 0, false))?;
            }
            Op::LoadI | Op::LoadF => {
                let k = if l.op == Op::LoadI {
                    Kind::LoadI
                } else {
                    Kind::LoadF
                };
                let a = self.asm.r(l.a)?;
                self.emit(self.asbx(k, a, l.sbx))?;
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
                self.emit(self.abc(Kind::LoadKx, a, 0, 0, false))?;
                self.emit(self.ax(x.ax() as u64))?;
                return Ok(2);
            }
            Op::LoadFalse | Op::LFalseSkip | Op::LoadTrue => {
                let k = match l.op {
                    Op::LoadFalse => Kind::LoadFalse,
                    Op::LFalseSkip => Kind::LFalseSkip,
                    _ => Kind::LoadTrue,
                };
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(k, a, 0, 0, false))?;
            }
            Op::LoadNil => {
                let a = self.asm.run(l.a, l.b + 1)?;
                self.emit(self.abc(Kind::LoadNil, a, l.b, 0, false))?;
            }
            Op::GetUpval | Op::SetUpval => {
                let k = if l.op == Op::GetUpval {
                    Kind::GetUpval
                } else {
                    Kind::SetUpval
                };
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(k, a, l.b, 0, false))?;
            }
            Op::GetTabUp => {
                let a = self.asm.r(l.a)?;
                if self.asm.short_str(l.c) {
                    self.emit(self.abc(Kind::GetTabUp, a, l.b, l.c, false))?;
                } else {
                    let (t, key) = (self.asm.temp()?, self.asm.temp()?);
                    self.emit(self.abc(Kind::GetUpval, t, l.b, 0, false))?;
                    self.load_k(key, l.c)?;
                    self.emit(self.abc(Kind::GetTable, a, t, key, false))?;
                }
            }
            Op::GetTable => {
                let (a, b, c) = (self.asm.r(l.a)?, self.asm.r(l.b)?, self.asm.r(l.c)?);
                self.emit(self.abc(Kind::GetTable, a, b, c, false))?;
            }
            Op::GetI => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::GetI, a, b, l.c, false))?;
            }
            Op::GetField => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                if self.asm.short_str(l.c) {
                    self.emit(self.abc(Kind::GetField, a, b, l.c, false))?;
                } else {
                    let key = self.asm.temp()?;
                    self.load_k(key, l.c)?;
                    self.emit(self.abc(Kind::GetTable, a, b, key, false))?;
                }
            }
            Op::SetTabUp => {
                let c = self.asm.r(l.c)?;
                if self.asm.short_str(l.b) {
                    self.emit(self.abc(Kind::SetTabUp, l.a, l.b, c, false))?;
                } else {
                    let (t, key) = (self.asm.temp()?, self.asm.temp()?);
                    self.emit(self.abc(Kind::GetUpval, t, l.a, 0, false))?;
                    self.load_k(key, l.b)?;
                    self.emit(self.abc(Kind::SetTable, t, key, c, false))?;
                }
            }
            Op::SetTable => {
                let (a, b, c) = (self.asm.r(l.a)?, self.asm.r(l.b)?, self.asm.r(l.c)?);
                self.emit(self.abc(Kind::SetTable, a, b, c, false))?;
            }
            Op::SetI => {
                let (a, c) = (self.asm.r(l.a)?, self.asm.r(l.c)?);
                self.emit(self.abc(Kind::SetI, a, l.b, c, false))?;
            }
            Op::SetField => {
                let (a, c) = (self.asm.r(l.a)?, self.asm.r(l.c)?);
                if self.asm.short_str(l.b) {
                    self.emit(self.abc(Kind::SetField, a, l.b, c, false))?;
                } else {
                    let key = self.asm.temp()?;
                    self.load_k(key, l.b)?;
                    self.emit(self.abc(Kind::SetTable, a, key, c, false))?;
                }
            }
            Op::NewTable => self.new_table(l)?,
            Op::SelfOp => self.self_op(l)?,
            Op::Add if l.k => {
                // `x - 0`: PUC's `ADDI x 0` with `__sub` recorded on its MMBINI
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::ArithI, a, b, 127, false))?;
                self.emit(self.abc(Kind::MmBinI, b, 127, 7, false))?;
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
                let tm = event(l.op).expect("arithmetic op");
                self.emit(self.abc(Kind::Arith(l.op), a, b, c, false))?;
                self.emit(self.abc(Kind::MmBin, b, c, tm, false))?;
            }
            Op::AddI | Op::SubI | Op::ShrI | Op::ShlI => self.arith_i(l)?,
            op if op.arith_const_op().is_some() => self.arith_k(l)?,
            Op::Unm | Op::BNot | Op::Not | Op::Len => {
                let (a, b) = (self.asm.r(l.a)?, self.asm.r(l.b)?);
                self.emit(self.abc(Kind::Unary(l.op), a, b, 0, false))?;
            }
            Op::Concat => {
                let a = self.asm.run(l.a, l.b.max(1))?;
                self.emit(self.abc(Kind::Concat, a, l.b, 0, false))?;
            }
            Op::Close | Op::Tbc => {
                let k = if l.op == Op::Close {
                    Kind::Close
                } else {
                    Kind::Tbc
                };
                let a = self.asm.r(l.a)?;
                self.emit(self.abc(k, a, 0, 0, false))?;
            }
            _ => return self.flow(l, caps),
        }
        Ok(1)
    }
}

/// Encode `asm.p`'s code. `skip_first` drops luna's leading `GetVarg`,
/// whose table 5.5's `VARARGPREP` builds itself (`PF_VATAB`).
pub(super) fn encode(asm: &mut Asm, f: &Frame, skip_first: bool, caps: &mut Caps) -> Res<()> {
    let p = asm.p;
    if p.is_vararg {
        let a = if f.v55 { 0 } else { f.np };
        let w = opn(f.ops, Kind::VarargPrep) | (a << 7);
        asm.emit_prologue(w, p.line_defined.max(1));
    }
    let mut pc = usize::from(skip_first);
    while pc < p.code.len() {
        asm.begin(pc);
        let l = L::of(p.code[pc]);
        pc += M { asm, f }.one(l, caps)?;
    }
    Ok(())
}
