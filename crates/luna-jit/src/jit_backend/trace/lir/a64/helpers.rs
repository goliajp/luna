//! Encoding helpers the AArch64 instructions share.

use super::*;

impl A64 {
    pub(super) fn put(&mut self, w: u32) {
        self.code.push(w);
    }

    pub(super) fn sf(wide: bool) -> u32 {
        u32::from(wide) << 31
    }

    pub(super) fn cond(c: Cond) -> u32 {
        match c {
            Cond::Eq => 0,
            Cond::Ne => 1,
            Cond::Uge => 2,
            Cond::Ult => 3,
            Cond::Ugt => 8,
            Cond::Ule => 9,
            Cond::Sge => 10,
            Cond::Slt => 11,
            Cond::Sgt => 12,
            Cond::Sle => 13,
        }
    }

    /// `rd = rn + imm` for any `imm`, `rn` may be the stack pointer.
    pub(super) fn add_any(&mut self, rd: u8, rn: u8, imm: i64) {
        let (rd, rn) = (u32::from(rd), u32::from(rn));
        if (0..4096).contains(&imm) {
            self.put(0x9100_0000 | ((imm as u32) << 10) | (rn << 5) | rd);
        } else if (-4095..0).contains(&imm) {
            self.put(0xD100_0000 | (((-imm) as u32) << 10) | (rn << 5) | rd);
        } else {
            self.mov_imm(TMP, imm);
            // add (extended register, uxtx): rn may be sp
            self.put(0x8B20_6000 | (u32::from(TMP) << 16) | (rn << 5) | rd);
        }
    }

    /// Load or store `rt` at `[rn + off]`: `scaled` is the unsigned-offset
    /// opcode, `unscaled` the 9-bit signed one, `reg` the register-offset one.
    pub(super) fn mem(
        &mut self,
        scaled: u32,
        unscaled: u32,
        reg: u32,
        size: i32,
        rt: u8,
        rn: u8,
        off: i32,
    ) {
        let (rt, rn) = (u32::from(rt), u32::from(rn));
        if off >= 0 && off % size == 0 && off / size < 4096 {
            self.put(scaled | (((off / size) as u32) << 10) | (rn << 5) | rt);
        } else if (-256..256).contains(&off) {
            self.put(unscaled | (((off as u32) & 0x1ff) << 12) | (rn << 5) | rt);
        } else {
            self.mov_imm(TMP, i64::from(off));
            self.put(reg | (u32::from(TMP) << 16) | (rn << 5) | rt);
        }
    }

    pub(super) fn branch_fix(&mut self, l: Label, kind: u8) {
        self.fixups.push((self.code.len() as u32, l.0, kind));
    }

    pub(super) fn save_restore(&mut self, store: bool) {
        let base = self.locals as i32;
        let n = self.saved.len();
        for k in 0..n {
            let (r, off) = (self.saved[k], base + 8 * k as i32);
            if store {
                self.store(Width::B8, r, Self::SP, off);
            } else {
                self.load(Width::B8, r, Self::SP, off);
            }
        }
        for k in 0..self.fsaved.len() {
            let (r, off) = (self.fsaved[k], base + 8 * (n + k) as i32);
            if store {
                self.fstore(r, Self::SP, off);
            } else {
                self.fload(r, Self::SP, off);
            }
        }
    }
}
