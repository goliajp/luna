//! Integer arithmetic: register and immediate forms.

use super::*;

impl X64 {
    pub(super) fn alu_rr(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8) {
        let mov = |s: &mut Self, d: u8, a: u8| {
            if wide {
                s.mov(d, a)
            } else if d != a {
                s.mov32(d, a)
            }
        };
        match op {
            Alu::Add | Alu::And | Alu::Or | Alu::Xor | Alu::Sub => {
                let opc = match op {
                    Alu::Add => 0x01,
                    Alu::And => 0x21,
                    Alu::Or => 0x09,
                    Alu::Xor => 0x31,
                    _ => 0x29,
                };
                if d == b && d != a {
                    if op == Alu::Sub {
                        // d = a - d
                        self.op(&[], wide, &[0xF7], 3, Rm::Reg(d), false);
                        self.op(&[], wide, &[0x01], a, Rm::Reg(d), false);
                    } else {
                        self.op(&[], wide, &[opc], a, Rm::Reg(d), false);
                    }
                    return;
                }
                mov(self, d, a);
                self.op(&[], wide, &[opc], b, Rm::Reg(d), false);
            }
            Alu::Mul => {
                let (x, y) = if d == b { (b, a) } else { (a, b) };
                mov(self, d, x);
                self.op(&[], wide, &[0x0F, 0xAF], d, Rm::Reg(y), false);
            }
            Alu::Shl | Alu::Lshr | Alu::Ashr => {
                let ext = match op {
                    Alu::Shl => 4,
                    Alu::Lshr => 5,
                    _ => 7,
                };
                self.mov(RCX, b);
                mov(self, d, a);
                self.op(&[], wide, &[0xD3], ext, Rm::Reg(d), false);
            }
            Alu::Sdiv | Alu::Udiv | Alu::Umulhi => {
                self.mov(RAX, a);
                // sign-extend rax into rdx for idiv, zero rdx for div; mul
                // overwrites rdx
                match op {
                    Alu::Sdiv => {
                        if wide {
                            self.code.push(0x48);
                        }
                        self.code.push(0x99);
                    }
                    // xor edx, edx
                    Alu::Udiv => self.code.extend_from_slice(&[0x31, 0xD2]),
                    _ => {}
                }
                let (ext, r) = match op {
                    Alu::Sdiv => (7, RAX),
                    Alu::Udiv => (6, RAX),
                    _ => (4, RDX),
                };
                self.op(&[], wide, &[0xF7], ext, Rm::Reg(b), false);
                if wide {
                    self.mov(d, r);
                } else {
                    self.mov32(d, r);
                }
            }
        }
    }
    pub(super) fn alu_ri(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool {
        let imm = if wide { imm } else { i64::from(imm as i32) };
        let Ok(v) = i32::try_from(imm) else {
            return false;
        };
        let mov = |s: &mut Self| {
            if wide {
                s.mov(d, a)
            } else if d != a {
                s.mov32(d, a)
            }
        };
        match op {
            Alu::Add | Alu::Or | Alu::And | Alu::Sub | Alu::Xor => {
                let ext = match op {
                    Alu::Add => 0,
                    Alu::Or => 1,
                    Alu::And => 4,
                    Alu::Sub => 5,
                    _ => 6,
                };
                mov(self);
                self.grp1(wide, ext, d, v);
            }
            Alu::Shl | Alu::Lshr | Alu::Ashr => {
                let ext = match op {
                    Alu::Shl => 4,
                    Alu::Lshr => 5,
                    _ => 7,
                };
                mov(self);
                self.op(&[], wide, &[0xC1], ext, Rm::Reg(d), false);
                self.code.push((v & if wide { 63 } else { 31 }) as u8);
            }
            Alu::Mul => {
                self.op(&[], wide, &[0x69], d, Rm::Reg(a), false);
                self.imm32(v);
            }
            Alu::Sdiv | Alu::Udiv | Alu::Umulhi => return false,
        }
        true
    }
}
