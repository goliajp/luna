//! Instruction encoding helpers.

use super::*;

impl X64 {
    /// `[prefix] [REX] opcode modrm [sib] [disp]`; `byte_reg` forces a REX
    /// so that registers 4-7 mean spl / bpl / sil / dil.
    pub(super) fn op(
        &mut self,
        prefix: &[u8],
        w: bool,
        opc: &[u8],
        reg: u8,
        rm: Rm,
        byte_reg: bool,
    ) {
        self.code.extend_from_slice(prefix);
        let b = match rm {
            Rm::Reg(r) | Rm::Mem(r, _) => r,
        };
        let rex = 0x40 | (u8::from(w) << 3) | ((reg >> 3) << 2) | (b >> 3);
        let byte_needs =
            byte_reg && ((4..8).contains(&reg) || matches!(rm, Rm::Reg(r) if (4..8).contains(&r)));
        if rex != 0x40 || byte_needs {
            self.code.push(rex);
        }
        self.code.extend_from_slice(opc);
        match rm {
            Rm::Reg(r) => self.code.push(0xC0 | ((reg & 7) << 3) | (r & 7)),
            Rm::Mem(base, disp) => {
                let md = if disp == 0 && base & 7 != 5 {
                    0
                } else if (-128..128).contains(&disp) {
                    1
                } else {
                    2
                };
                self.code.push((md << 6) | ((reg & 7) << 3) | (base & 7));
                if base & 7 == 4 {
                    self.code.push(0x24);
                }
                match md {
                    1 => self.code.push(disp as u8),
                    2 => self.code.extend_from_slice(&disp.to_le_bytes()),
                    _ => {}
                }
            }
        }
    }

    pub(super) fn imm32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    pub(super) fn cc(c: Cond) -> u8 {
        match c {
            Cond::Eq => 4,
            Cond::Ne => 5,
            Cond::Slt => 12,
            Cond::Sle => 14,
            Cond::Sgt => 15,
            Cond::Sge => 13,
            Cond::Ult => 2,
            Cond::Ule => 6,
            Cond::Ugt => 7,
            Cond::Uge => 3,
        }
    }

    pub(super) fn mov32(&mut self, d: u8, s: u8) {
        self.op(&[], false, &[0x89], s, Rm::Reg(d), false);
    }

    pub(super) fn jcc_raw(&mut self, cc: u8, l: Label) {
        self.code.extend_from_slice(&[0x0F, 0x80 | cc]);
        self.fixups.push((self.code.len() as u32, l.0, 0));
        self.imm32(0);
    }

    pub(super) fn set8(&mut self, cc: u8, d: u8) {
        self.op(&[], false, &[0x0F, 0x90 | cc], 0, Rm::Reg(d), true);
    }

    pub(super) fn movzx8(&mut self, d: u8, s: u8) {
        self.op(&[], false, &[0x0F, 0xB6], d, Rm::Reg(s), true);
    }

    pub(super) fn sse(&mut self, prefix: u8, opc: u8, d: u8, s: u8) {
        self.op(&[prefix], false, &[0x0F, opc], d, Rm::Reg(s), false);
    }

    pub(super) fn movaps(&mut self, d: u8, s: u8) {
        if d != s {
            self.op(&[], false, &[0x0F, 0x28], d, Rm::Reg(s), false);
        }
    }

    pub(super) fn ucomisd(&mut self, a: u8, b: u8) {
        self.sse(0x66, 0x2E, a, b);
    }

    /// `op rm, imm` (group 1: add 0, or 1, and 4, sub 5, xor 6, cmp 7).
    pub(super) fn grp1(&mut self, w: bool, ext: u8, r: u8, imm: i32) {
        if (-128..128).contains(&imm) {
            self.op(&[], w, &[0x83], ext, Rm::Reg(r), false);
            self.code.push(imm as u8);
        } else {
            self.op(&[], w, &[0x81], ext, Rm::Reg(r), false);
            self.imm32(imm);
        }
    }
}
