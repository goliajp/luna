//! AArch64 encoding of [`Masm`] (AAPCS64; x18 is left alone for Apple and
//! Windows, which reserve it).

use super::alloc::Class;
use super::cg::{Alu, Bufs, Cond, Label, Masm, Width};
use super::*;

mod imm;
use imm::logical_imm;

const XZR: u32 = 31;
const S0: u8 = 16;
const S1: u8 = 17;
/// Holds an indirect call target, and an offset too large for an
/// immediate.
const TMP: u8 = 15;

pub(crate) struct A64 {
    code: Vec<u32>,
    bytes: Vec<u8>,
    labels: Vec<u32>,
    /// (instruction index, label, kind: 0 = b, 1 = b.cond / cbz / cbnz)
    fixups: Vec<(u32, u32, u8)>,
    sites: Vec<crate::jit_backend::trace::reloc::Site>,
    /// `ldr` (literal) instructions loading relocation `n`, by index: the
    /// addresses go in a pool after the code
    lits: Vec<(u32, u32, i64)>,
    saved: Vec<u8>,
    fsaved: Vec<u8>,
    locals: u32,
    total: u32,
}

impl A64 {
    fn put(&mut self, w: u32) {
        self.code.push(w);
    }

    fn sf(wide: bool) -> u32 {
        u32::from(wide) << 31
    }

    fn cond(c: Cond) -> u32 {
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
    fn add_any(&mut self, rd: u8, rn: u8, imm: i64) {
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
    fn mem(&mut self, scaled: u32, unscaled: u32, reg: u32, size: i32, rt: u8, rn: u8, off: i32) {
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

    fn branch_fix(&mut self, l: Label, kind: u8) {
        self.fixups.push((self.code.len() as u32, l.0, kind));
    }

    fn save_restore(&mut self, store: bool) {
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

impl Masm for A64 {
    const SP: u8 = 31;
    const INT: Class = Class {
        caller: &[9, 10, 11, 12, 13, 14, 0, 1, 2, 3, 4, 5, 6, 7, 8],
        callee: &[19, 20, 21, 22, 23, 24, 25, 26, 27, 28],
    };
    const FLT: Class = Class {
        caller: &[
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 0, 1, 2, 3, 4, 5, 6, 7,
        ],
        callee: &[8, 9, 10, 11, 12, 13, 14, 15],
    };
    const SCRATCH: [u8; 2] = [S0, S1];
    const FSCRATCH: [u8; 2] = [30, 31];
    const CALL_TARGET: u8 = TMP;
    const INT_ARGS: &'static [u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
    const FLOAT_ARGS: &'static [u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
    const RET: u8 = 0;
    const FRET: u8 = 0;
    const CALL_SHADOW: u32 = 0;
    const POSITIONAL_ARGS: bool = false;

    fn new_label(&mut self) -> Label {
        self.labels.push(NONE);
        Label(self.labels.len() as u32 - 1)
    }
    fn bind(&mut self, l: Label) {
        self.labels[l.0 as usize] = self.code.len() as u32;
    }
    fn jmp(&mut self, l: Label) {
        self.branch_fix(l, 0);
        self.put(0x1400_0000);
    }
    fn jcc(&mut self, c: Cond, l: Label) {
        self.branch_fix(l, 1);
        self.put(0x5400_0000 | Self::cond(c));
    }
    fn branch_reg(&mut self, r: u8, nz: bool, l: Label) {
        self.branch_fix(l, 1);
        self.put(if nz { 0xB500_0000 } else { 0xB400_0000 } | u32::from(r));
    }

    fn mov(&mut self, d: u8, s: u8) {
        if d != s {
            self.put(0xAA00_03E0 | (u32::from(s) << 16) | u32::from(d));
        }
    }
    fn mov_imm(&mut self, d: u8, v: i64) {
        let d = u32::from(d);
        let u = v as u64;
        let chunks = |x: u64| {
            (0..4)
                .filter(move |&k| (x >> (16 * k)) & 0xffff != 0)
                .count()
        };
        let invert = chunks(!u) < chunks(u);
        let base = if invert { !u } else { u };
        let mut first = true;
        for k in 0..4u32 {
            let c = ((base >> (16 * k)) & 0xffff) as u32;
            if c == 0 && !(first && k == 3) {
                continue;
            }
            if first {
                let op = if invert { 0x9280_0000 } else { 0xD280_0000 };
                self.put(op | (k << 21) | (c << 5) | d);
                first = false;
            } else {
                let c = ((u >> (16 * k)) & 0xffff) as u32;
                self.put(0xF280_0000 | (k << 21) | (c << 5) | d);
            }
        }
    }
    fn mov_reloc(&mut self, d: u8, v: i64, n: u32) {
        // ldr xd, <literal>: one load from the pool `finish` places after
        // the code, where another Vm's address is written over this one
        self.lits.push((self.code.len() as u32, n, v));
        self.put(0x5800_0000 | u32::from(d));
    }
    fn fmov(&mut self, d: u8, s: u8) {
        if d != s {
            self.put(0x1E60_4000 | (u32::from(s) << 5) | u32::from(d));
        }
    }
    fn bits_to_f(&mut self, d: u8, s: u8) {
        self.put(0x9E67_0000 | (u32::from(s) << 5) | u32::from(d));
    }
    fn bits_to_i(&mut self, d: u8, s: u8) {
        self.put(0x9E66_0000 | (u32::from(s) << 5) | u32::from(d));
    }
    fn load(&mut self, w: Width, d: u8, base: u8, off: i32) {
        match w {
            Width::B1 => self.mem(0x3940_0000, 0x3840_0000, 0x3860_6800, 1, d, base, off),
            Width::B2 => self.mem(0x7940_0000, 0x7840_0000, 0x7860_6800, 2, d, base, off),
            Width::B4 => self.mem(0xB940_0000, 0xB840_0000, 0xB860_6800, 4, d, base, off),
            Width::B8 => self.mem(0xF940_0000, 0xF840_0000, 0xF860_6800, 8, d, base, off),
        }
    }
    fn store(&mut self, w: Width, s: u8, base: u8, off: i32) {
        match w {
            Width::B1 => self.mem(0x3900_0000, 0x3800_0000, 0x3820_6800, 1, s, base, off),
            Width::B2 => self.mem(0x7900_0000, 0x7800_0000, 0x7820_6800, 2, s, base, off),
            Width::B4 => self.mem(0xB900_0000, 0xB800_0000, 0xB820_6800, 4, s, base, off),
            Width::B8 => self.mem(0xF900_0000, 0xF800_0000, 0xF820_6800, 8, s, base, off),
        }
    }
    fn fload(&mut self, d: u8, base: u8, off: i32) {
        self.mem(0xFD40_0000, 0xFC40_0000, 0xFC60_6800, 8, d, base, off);
    }
    fn fstore(&mut self, s: u8, base: u8, off: i32) {
        self.mem(0xFD00_0000, 0xFC00_0000, 0xFC20_6800, 8, s, base, off);
    }
    fn lea(&mut self, d: u8, base: u8, off: i32) {
        self.add_any(d, base, i64::from(off));
    }

    fn alu(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8) {
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
            Alu::Shl => 0x1AC0_2000,
            Alu::Lshr => 0x1AC0_2400,
            Alu::Ashr => 0x1AC0_2800,
        };
        self.put(sf | w | (b << 16) | (a << 5) | d);
    }
    fn alu_imm(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool {
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
            Alu::Mul | Alu::Sdiv | Alu::Udiv => false,
        }
    }
    fn neg(&mut self, wide: bool, d: u8, a: u8) {
        self.put(Self::sf(wide) | 0x4B00_0000 | (u32::from(a) << 16) | (XZR << 5) | u32::from(d));
    }
    fn not(&mut self, wide: bool, d: u8, a: u8) {
        self.put(Self::sf(wide) | 0x2A20_0000 | (u32::from(a) << 16) | (XZR << 5) | u32::from(d));
    }
    fn cmp(&mut self, wide: bool, a: u8, b: u8) {
        self.put(Self::sf(wide) | 0x6B00_0000 | (u32::from(b) << 16) | (u32::from(a) << 5) | XZR);
    }
    fn cmp_imm(&mut self, wide: bool, a: u8, imm: i64) -> bool {
        let (opc, m) = if imm >= 0 {
            (0x7100_0000, imm)
        } else {
            (0x3100_0000, imm.wrapping_neg())
        };
        if !(0..4096).contains(&m) {
            return false;
        }
        self.put(Self::sf(wide) | opc | ((m as u32) << 10) | (u32::from(a) << 5) | XZR);
        true
    }
    fn setcc(&mut self, d: u8, c: Cond) {
        let inv = Self::cond(c.invert());
        self.put(0x9A80_0400 | (XZR << 16) | (inv << 12) | (XZR << 5) | u32::from(d));
    }
    fn csel(&mut self, d: u8, c: Cond, a: u8, b: u8) {
        let w = 0x9A80_0000 | (u32::from(b) << 16) | (Self::cond(c) << 12);
        self.put(w | (u32::from(a) << 5) | u32::from(d));
    }
    fn zext(&mut self, d: u8, a: u8, bits: u32) {
        let (d, a) = (u32::from(d), u32::from(a));
        match bits {
            8 => self.put(0x5300_1C00 | (a << 5) | d),
            16 => self.put(0x5300_3C00 | (a << 5) | d),
            32 => self.put(0x2A00_03E0 | (a << 16) | d),
            _ => self.mov(d as u8, a as u8),
        }
    }
    fn sext(&mut self, d: u8, a: u8, bits: u32) {
        let (d, a) = (u32::from(d), u32::from(a));
        match bits {
            8 => self.put(0x9340_1C00 | (a << 5) | d),
            16 => self.put(0x9340_3C00 | (a << 5) | d),
            32 => self.put(0x9340_7C00 | (a << 5) | d),
            _ => self.mov(d as u8, a as u8),
        }
    }

    fn fbin(&mut self, op: BinOp, d: u8, a: u8, b: u8) {
        let w = match op {
            BinOp::Fadd => 0x1E60_2800,
            BinOp::Fsub => 0x1E60_3800,
            BinOp::Fmul => 0x1E60_0800,
            _ => 0x1E60_1800,
        };
        self.put(w | (u32::from(b) << 16) | (u32::from(a) << 5) | u32::from(d));
    }
    fn fneg(&mut self, d: u8, a: u8) {
        self.put(0x1E61_4000 | (u32::from(a) << 5) | u32::from(d));
    }
    fn fround(&mut self, d: u8, a: u8, up: bool) -> bool {
        let w = if up { 0x1E64_C000 } else { 0x1E65_4000 };
        self.put(w | (u32::from(a) << 5) | u32::from(d));
        true
    }
    fn i2f(&mut self, d: u8, a: u8) {
        self.put(0x9E62_0000 | (u32::from(a) << 5) | u32::from(d));
    }
    fn f2i(&mut self, d: u8, a: u8) {
        self.put(0x9E78_0000 | (u32::from(a) << 5) | u32::from(d));
    }
    fn f2i_sat(&mut self, d: u8, a: u8) {
        // fcvtzs saturates and turns NaN into 0
        self.f2i(d, a);
    }
    fn fcmp_set(&mut self, d: u8, cc: FloatCC, a: u8, b: u8) -> bool {
        // after fcmp an unordered result sets C and V
        let c = match cc {
            FloatCC::Equal => 0,
            FloatCC::NotEqual => 1,
            FloatCC::LessThan => 4,
            FloatCC::LessThanOrEqual => 9,
            FloatCC::GreaterThan => 12,
            FloatCC::GreaterThanOrEqual => 10,
            _ => return false,
        };
        self.put(0x1E60_2000 | (u32::from(b) << 16) | (u32::from(a) << 5));
        let inv = c ^ 1;
        self.put(0x9A80_0400 | (XZR << 16) | (inv << 12) | (XZR << 5) | u32::from(d));
        true
    }

    fn call_abs(&mut self, addr: usize) {
        self.mov_imm(S0, addr as i64);
        self.call_reg(S0);
    }
    fn call_reg(&mut self, r: u8) {
        self.put(0xD63F_0000 | (u32::from(r) << 5));
    }
    fn prologue(&mut self, saved: &[u8], fsaved: &[u8], locals: u32) {
        self.saved.clear();
        self.saved.extend_from_slice(saved);
        self.fsaved.clear();
        self.fsaved.extend_from_slice(fsaved);
        self.locals = locals;
        let save = 8 * (saved.len() + fsaved.len()) as u32;
        self.total = locals + save.div_ceil(16) * 16;
        // stp x29, x30, [sp, #-16]!; mov x29, sp
        self.put(0xA9BF_7BFD);
        self.put(0x9100_03FD);
        let t = i64::from(self.total);
        self.add_any(31, 31, -t);
        self.save_restore(true);
    }
    fn epilogue_ret(&mut self) {
        self.save_restore(false);
        let t = i64::from(self.total);
        self.add_any(31, 31, t);
        // ldp x29, x30, [sp], #16; ret
        self.put(0xA8C1_7BFD);
        self.put(0xD65F_03C0);
    }
    fn new(b: Bufs) -> A64 {
        let Bufs {
            mut bytes,
            words: mut code,
            mut labels,
            mut fixups,
            mut sites,
        } = b;
        bytes.clear();
        code.clear();
        labels.clear();
        fixups.clear();
        sites.clear();
        A64 {
            code,
            bytes,
            labels,
            fixups,
            sites,
            lits: Vec::new(),
            saved: Vec::new(),
            fsaved: Vec::new(),
            locals: 0,
            total: 0,
        }
    }
    fn finish(mut self) -> Bufs {
        for &(at, l, kind) in &self.fixups {
            let target = self.labels[l as usize];
            let delta = target as i64 - i64::from(at);
            let w = &mut self.code[at as usize];
            if kind == 0 {
                *w |= (delta as u32) & 0x03FF_FFFF;
            } else {
                *w |= ((delta as u32) & 0x7FFFF) << 5;
            }
        }
        if !self.lits.is_empty() && self.code.len() % 2 == 1 {
            // nop: the pool's eight-byte words start eight-byte aligned
            self.put(0xD503_201F);
        }
        let mut pool: Vec<(u32, u32)> = Vec::new();
        for &(at, n, v) in &self.lits {
            let lit = match pool.iter().find(|p| p.0 == n) {
                Some(&(_, w)) => w,
                None => {
                    let w = self.code.len() as u32;
                    self.code.push(v as u32);
                    self.code.push((v as u64 >> 32) as u32);
                    self.sites
                        .push(crate::jit_backend::trace::reloc::Site { at: 4 * w, n });
                    pool.push((n, w));
                    w
                }
            };
            self.code[at as usize] |= ((lit - at) & 0x7FFFF) << 5;
        }
        for w in &self.code {
            self.bytes.extend_from_slice(&w.to_le_bytes());
        }
        Bufs {
            bytes: self.bytes,
            words: self.code,
            labels: self.labels,
            fixups: self.fixups,
            sites: self.sites,
        }
    }
}
