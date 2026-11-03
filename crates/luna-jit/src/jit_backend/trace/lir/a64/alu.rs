//! Integer arithmetic: register and immediate forms.

use super::*;

impl A64 {
    pub(super) fn alu_rr(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8) {
        let (d, a, b) = (u32::from(d), u32::from(a), u32::from(b));
        let sf = Self::sf(wide);
        let w = match op {
            Alu::Add => 0x0B00_0000,
            Alu::Sub => 0x4B00_0000,
            Alu::And => 0x0A00_0000,
            Alu::Or => 0x2A00_0000,
            Alu::Xor => 0x4A00_0000,
            Alu::Mul => 0x1B00_0000 | (XZR << 10),
            Alu::Sdiv => 0x1AC0_0C00,
            Alu::Udiv => 0x1AC0_0800,
            Alu::Umulhi => 0x9BC0_7C00,
            Alu::Shl => 0x1AC0_2000,
            Alu::Lshr => 0x1AC0_2400,
            Alu::Ashr => 0x1AC0_2800,
        };
        self.put(sf | w | (b << 16) | (a << 5) | d);
    }
    pub(super) fn alu_ri(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool {
        let (d, a) = (u32::from(d), u32::from(a));
        let sf = Self::sf(wide);
        let bits = if wide { 64 } else { 32 };
        let imm = if wide { imm } else { i64::from(imm as i32) };
        match op {
            Alu::Add | Alu::Sub => {
                let neg = op == Alu::Sub;
                let v = if neg { imm.wrapping_neg() } else { imm };
                let (opc, m) = if v >= 0 {
                    (0x1100_0000, v)
                } else {
                    (0x5100_0000, v.wrapping_neg())
                };
                if (0..4096).contains(&m) {
                    self.put(sf | opc | ((m as u32) << 10) | (a << 5) | d);
                } else if m & 0xfff == 0 && (0..4096).contains(&(m >> 12)) {
                    self.put(sf | opc | (1 << 22) | (((m >> 12) as u32) << 10) | (a << 5) | d);
                } else {
                    return false;
                }
                true
            }
            Alu::And | Alu::Or | Alu::Xor => {
                let Some(e) = logical_imm(imm as u64, bits) else {
                    return false;
                };
                let opc = match op {
                    Alu::And => 0x1200_0000,
                    Alu::Or => 0x3200_0000,
                    _ => 0x5200_0000,
                };
                self.put(sf | opc | (e << 10) | (a << 5) | d);
                true
            }
            Alu::Shl | Alu::Lshr | Alu::Ashr => {
                let s = (imm as u32) & (bits - 1);
                let (n, top) = if wide { (1 << 22, 63) } else { (0, 31) };
                let (opc, immr, imms) = match op {
                    Alu::Shl => (0x5300_0000, (bits - s) & (bits - 1), top - s),
                    Alu::Lshr => (0x5300_0000, s, top),
                    _ => (0x1300_0000, s, top),
                };
                self.put(sf | opc | n | (immr << 16) | (imms << 10) | (a << 5) | d);
                true
            }
            Alu::Mul | Alu::Sdiv | Alu::Udiv | Alu::Umulhi => false,
        }
    }
}
