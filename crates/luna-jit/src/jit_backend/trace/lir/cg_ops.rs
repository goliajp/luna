//! Code for each recorded instruction.

use super::cg::{Alu, Cond, Gen, Masm, Width};
use super::*;

type R = Result<(), &'static str>;

fn pure(op: Op) -> bool {
    !matches!(
        op,
        Op::Store(_)
            | Op::StackStore(..)
            | Op::Jump
            | Op::Brif(_)
            | Op::TierCount { .. }
            | Op::Call
            | Op::CallIndirect
            | Op::Return
            | Op::VarWrite
    )
}

fn alu_of(op: BinOp) -> Alu {
    match op {
        BinOp::Add => Alu::Add,
        BinOp::Sub => Alu::Sub,
        BinOp::Mul => Alu::Mul,
        BinOp::And => Alu::And,
        BinOp::Or => Alu::Or,
        BinOp::Xor => Alu::Xor,
        BinOp::Shl => Alu::Shl,
        BinOp::Ushr => Alu::Lshr,
        BinOp::Sshr => Alu::Ashr,
        BinOp::Sdiv => Alu::Sdiv,
        BinOp::Umulhi => Alu::Umulhi,
        BinOp::Smin | BinOp::Smax | BinOp::Fadd | BinOp::Fsub | BinOp::Fmul | BinOp::Fdiv => {
            unreachable!("not a single ALU instruction")
        }
    }
}

fn is_float_op(op: BinOp) -> bool {
    matches!(op, BinOp::Fadd | BinOp::Fsub | BinOp::Fmul | BinOp::Fdiv)
}

impl<M: Masm> Gen<'_, M> {
    pub(crate) fn inst(&mut self, ii: u32, peek: Option<u32>, next: u32) -> R {
        let i = self.lir.insts[ii as usize];
        if i.dst != NONE && self.an.uses[i.dst as usize] == 0 && pure(i.op) {
            return Ok(());
        }
        let nv = self.an.n_values;
        match i.op {
            Op::Iconst(n) => {
                let d = self.dst(i.dst, 0);
                self.m.mov_imm(d, n);
                self.commit(i.dst, d);
            }
            Op::Reloc(n) => {
                let d = self.dst(i.dst, 0);
                self.m.mov_reloc(d, self.lir.relocs[n as usize].1, n);
                self.commit(i.dst, d);
            }
            Op::Fconst(b) => {
                self.m.mov_imm(M::SCRATCH[0], b as i64);
                let d = self.fdst(i.dst, 0);
                self.m.bits_to_f(d, M::SCRATCH[0]);
                self.fcommit(i.dst, d);
            }
            Op::Bin(op) if is_float_op(op) => {
                let a = self.fsrc(i.a, 0);
                let b = self.fsrc(i.b, 1);
                let d = self.fdst(i.dst, 0);
                self.m.fbin(op, d, a, b);
                self.fcommit(i.dst, d);
            }
            Op::Bin(op) => self.int_bin(op, &i, None)?,
            Op::BinImm(op, imm) => self.int_bin(op, &i, Some(imm))?,
            Op::Un(u) => self.unary(u, &i)?,
            Op::Icmp(cc) => self.icmp(cc, &i, None, peek)?,
            Op::IcmpImm(cc, imm) => self.icmp(cc, &i, Some(imm), peek)?,
            Op::Fcmp(cc) => {
                let a = self.fsrc(i.a, 0);
                let b = self.fsrc(i.b, 1);
                let d = self.dst(i.dst, 0);
                if !self.m.fcmp_set(d, cc, a, b) {
                    return Err("float condition");
                }
                self.commit(i.dst, d);
            }
            Op::Select => {
                if i.ty.is_float() {
                    return Err("float select");
                }
                // the condition may be any integer type; narrow ones are
                // held zero-extended, so a full-width test is right for all
                let c = self.src(i.a, 0);
                if !self.m.cmp_imm(true, c, 0) {
                    unreachable!("comparing with zero always encodes");
                }
                let x = self.src(i.b, 0);
                let y = self.src(i.c, 1);
                let d = self.dst(i.dst, 0);
                self.m.csel(d, Cond::Ne, x, y);
                self.commit(i.dst, d);
            }
            Op::Load(off) | Op::Uload8(off) => {
                let p = self.src(i.a, 0);
                if i.ty.is_float() {
                    let d = self.fdst(i.dst, 0);
                    self.m.fload(d, p, off);
                    self.fcommit(i.dst, d);
                } else {
                    let w = match i.op {
                        Op::Uload8(_) => Width::B1,
                        _ => Width::of(i.ty),
                    };
                    let d = self.dst(i.dst, 0);
                    self.m.load(w, d, p, off);
                    self.commit(i.dst, d);
                }
            }
            Op::Store(off) => {
                let p = self.src(i.b, 1);
                if i.ty.is_float() {
                    let x = self.fsrc(i.a, 0);
                    self.m.fstore(x, p, off);
                } else {
                    let x = self.src(i.a, 0);
                    self.m.store(Width::of(i.ty), x, p, off);
                }
            }
            Op::StackAddr(slot, off) => {
                let d = self.dst(i.dst, 0);
                let o = self.slot_off(slot) + off;
                self.m.lea(d, M::SP, o);
                self.commit(i.dst, d);
            }
            Op::StackLoad(slot, off) => {
                let o = self.slot_off(slot) + off;
                if i.ty.is_float() {
                    let d = self.fdst(i.dst, 0);
                    self.m.fload(d, M::SP, o);
                    self.fcommit(i.dst, d);
                } else {
                    let d = self.dst(i.dst, 0);
                    self.m.load(Width::of(i.ty), d, M::SP, o);
                    self.commit(i.dst, d);
                }
            }
            Op::StackStore(slot, off) => {
                let o = self.slot_off(slot) + off;
                if i.ty.is_float() {
                    let x = self.fsrc(i.a, 0);
                    self.m.fstore(x, M::SP, o);
                } else {
                    let x = self.src(i.a, 0);
                    self.m.store(Width::of(i.ty), x, M::SP, o);
                }
            }
            Op::Jump => {
                let args =
                    self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize].to_vec();
                self.jump(i.a, &args, next);
            }
            Op::Brif(n_then) => self.brif(&i, n_then, next),
            Op::TierCount { n, at } => self.tier_count(n, at, i.b, i.c, next),
            Op::Call | Op::CallIndirect => {
                let lir = self.lir;
                let (f, addr) = match i.op {
                    Op::Call => {
                        let f = &lir.funcs[i.a as usize];
                        (f, Some(f.addr))
                    }
                    _ => (&lir.sigs[i.b as usize], None),
                };
                let params = lir.params(f);
                if addr == Some(0) || Self::stack_args(params).is_none() {
                    return Err("call");
                }
                self.call(&i, addr, params);
            }
            Op::Return => {
                if i.a != NONE {
                    let r = self.src(i.a, 0);
                    self.m.mov(M::RET, r);
                }
                self.m.epilogue_ret();
            }
            Op::VarRead => self.copy(i.dst, nv + i.a),
            Op::VarWrite => self.copy(nv + i.a, i.b),
        }
        Ok(())
    }

    fn int_bin(&mut self, op: BinOp, i: &Inst, imm: Option<i64>) -> R {
        let ty = i.ty;
        let wide = ty == Ty::I64;
        let narrow = matches!(ty, Ty::I8 | Ty::I16);
        if narrow
            && !matches!(
                op,
                BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Add | BinOp::Sub | BinOp::Mul
            )
        {
            return Err("narrow integer operation");
        }
        let x = self.src(i.a, 0);
        let y = match imm {
            None => self.src(i.b, 1),
            Some(_) => M::SCRATCH[1],
        };
        let d = self.dst(i.dst, 0);
        match op {
            BinOp::Smin | BinOp::Smax => {
                if let Some(v) = imm {
                    self.m.mov_imm(y, v);
                }
                self.m.cmp(wide, x, y);
                let c = if op == BinOp::Smin {
                    Cond::Slt
                } else {
                    Cond::Sgt
                };
                self.m.csel(d, c, x, y);
            }
            _ => {
                let alu = alu_of(op);
                match imm {
                    Some(v) if self.m.alu_imm(alu, wide, d, x, v) => {}
                    Some(v) => {
                        self.m.mov_imm(y, v);
                        self.m.alu(alu, wide, d, x, y);
                    }
                    None => self.m.alu(alu, wide, d, x, y),
                }
            }
        }
        if narrow {
            self.m.zext(d, d, ty.bits());
        }
        self.commit(i.dst, d);
        Ok(())
    }

    fn unary(&mut self, u: UnOp, i: &Inst) -> R {
        let src_float = self.is_float(i.a);
        let src_ty = self.lir.value_ty[i.a as usize];
        match u {
            UnOp::Ineg | UnOp::Bnot => {
                let x = self.src(i.a, 0);
                let d = self.dst(i.dst, 0);
                let wide = i.ty == Ty::I64;
                if u == UnOp::Ineg {
                    self.m.neg(wide, d, x);
                } else {
                    self.m.not(wide, d, x);
                }
                if matches!(i.ty, Ty::I8 | Ty::I16) {
                    self.m.zext(d, d, i.ty.bits());
                }
                self.commit(i.dst, d);
            }
            UnOp::Fneg | UnOp::Floor | UnOp::Ceil => {
                let x = self.fsrc(i.a, 0);
                let d = self.fdst(i.dst, 0);
                match u {
                    UnOp::Fneg => self.m.fneg(d, x),
                    _ => {
                        if !self.m.fround(d, x, u == UnOp::Ceil) {
                            return Err("float rounding");
                        }
                    }
                }
                self.fcommit(i.dst, d);
            }
            UnOp::Uextend => self.copy(i.dst, i.a),
            UnOp::Ireduce => {
                let x = self.src(i.a, 0);
                let d = self.dst(i.dst, 0);
                self.m.zext(d, x, i.ty.bits());
                self.commit(i.dst, d);
            }
            UnOp::Bitcast => match (src_float, i.ty.is_float()) {
                (false, true) => {
                    let x = self.src(i.a, 0);
                    let d = self.fdst(i.dst, 0);
                    self.m.bits_to_f(d, x);
                    self.fcommit(i.dst, d);
                }
                (true, false) => {
                    let x = self.fsrc(i.a, 0);
                    let d = self.dst(i.dst, 0);
                    self.m.bits_to_i(d, x);
                    self.commit(i.dst, d);
                }
                _ => self.copy(i.dst, i.a),
            },
            UnOp::FcvtFromSint => {
                let mut x = self.src(i.a, 0);
                if src_ty != Ty::I64 {
                    self.m.sext(M::SCRATCH[0], x, src_ty.bits());
                    x = M::SCRATCH[0];
                }
                let d = self.fdst(i.dst, 0);
                self.m.i2f(d, x);
                self.fcommit(i.dst, d);
            }
            UnOp::FcvtToSint | UnOp::FcvtToSintSat => {
                if i.ty != Ty::I64 {
                    return Err("narrow float conversion");
                }
                let x = self.fsrc(i.a, 0);
                let d = self.dst(i.dst, 0);
                if u == UnOp::FcvtToSint {
                    self.m.f2i(d, x);
                } else {
                    self.m.f2i_sat(d, x);
                }
                self.commit(i.dst, d);
            }
        }
        Ok(())
    }

    fn icmp(&mut self, cc: IntCC, i: &Inst, imm: Option<i64>, peek: Option<u32>) -> R {
        let c = Cond::of(cc).ok_or("integer condition")?;
        let ty = i.ty;
        let wide = ty == Ty::I64;
        let mut x = self.src(i.a, 0);
        let mut y = match imm {
            None => self.src(i.b, 1),
            Some(_) => NONE as u8,
        };
        if matches!(ty, Ty::I8 | Ty::I16) && c.signed() {
            self.m.sext(M::SCRATCH[0], x, ty.bits());
            x = M::SCRATCH[0];
            if imm.is_none() {
                self.m.sext(M::SCRATCH[1], y, ty.bits());
                y = M::SCRATCH[1];
            }
        }
        match imm {
            Some(v) => {
                let v = if wide { v } else { i64::from(v as i32) };
                let v = match ty {
                    Ty::I8 if c.signed() => i64::from(v as i8),
                    Ty::I16 if c.signed() => i64::from(v as i16),
                    Ty::I8 => v & 0xff,
                    Ty::I16 => v & 0xffff,
                    _ => v,
                };
                if !self.m.cmp_imm(wide, x, v) {
                    self.m.mov_imm(M::SCRATCH[1], v);
                    self.m.cmp(wide, x, M::SCRATCH[1]);
                }
            }
            None => self.m.cmp(wide, x, y),
        }
        let fused = peek.is_some_and(|p| {
            let n = &self.lir.insts[p as usize];
            matches!(n.op, Op::Brif(_)) && n.a == i.dst && self.an.uses[i.dst as usize] == 1
        });
        if fused {
            self.pending = Some((i.dst, c));
        } else {
            let d = self.dst(i.dst, 0);
            self.m.setcc(d, c);
            self.commit(i.dst, d);
        }
        Ok(())
    }
}
