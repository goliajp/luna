//! x86-64 encoding of [`Masm`] (System V, or Win64 on Windows).
//!
//! rax, rcx and rdx are never allocated: division, shift counts and call
//! targets need them. r10 and r11 are the scratch registers.

use super::alloc::Class;
use super::cg::{Alu, Bufs, Cond, Label, Masm, Width};
use super::*;

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RSP: u8 = 4;
const R10: u8 = 10;
const R11: u8 = 11;
/// A third floating-point temporary, never allocated.
const XTMP: u8 = abi::XTMP;

mod enc;

#[derive(Clone, Copy)]
enum Rm {
    Reg(u8),
    Mem(u8, i32),
}

pub(crate) struct X64 {
    code: Vec<u8>,
    labels: Vec<u32>,
    /// (offset of a rel32 field, label, unused)
    fixups: Vec<(u32, u32, u8)>,
    words: Vec<u32>,
    sites: Vec<crate::jit_backend::trace::reloc::Site>,
    saved: Vec<u8>,
    frame: u32,
}

/// System V on Unix, Win64 on Windows.
#[cfg(not(windows))]
#[path = "x64/abi_sysv.rs"]
mod abi;
#[cfg(windows)]
#[path = "x64/abi_win64.rs"]
mod abi;

impl Masm for X64 {
    const SP: u8 = RSP;
    const INT: Class = Class {
        caller: abi::CALLER,
        callee: abi::CALLEE,
    };
    const FLT: Class = Class {
        caller: abi::FCALLER,
        callee: &[],
    };
    const SCRATCH: [u8; 2] = [R10, R11];
    const FSCRATCH: [u8; 2] = abi::FSCRATCH;
    const CALL_TARGET: u8 = RAX;
    const INT_ARGS: &'static [u8] = abi::ARGS;
    const FLOAT_ARGS: &'static [u8] = abi::FARGS;
    const RET: u8 = RAX;
    const FRET: u8 = 0;
    const CALL_SHADOW: u32 = abi::SHADOW;
    const POSITIONAL_ARGS: bool = abi::POSITIONAL;

    fn new_label(&mut self) -> Label {
        self.labels.push(NONE);
        Label(self.labels.len() as u32 - 1)
    }
    fn bind(&mut self, l: Label) {
        self.labels[l.0 as usize] = self.code.len() as u32;
    }
    fn align(&mut self, to: u32) {
        // the multi-byte forms of `nop`, by length
        const NOPS: [&[u8]; 4] = [
            &[0x90],
            &[0x66, 0x90],
            &[0x0f, 0x1f, 0x00],
            &[0x0f, 0x1f, 0x40, 0x00],
        ];
        while self.code.len() as u32 % to != 0 {
            let k = ((to - self.code.len() as u32 % to) as usize).min(NOPS.len());
            self.code.extend_from_slice(NOPS[k - 1]);
        }
    }
    fn jmp(&mut self, l: Label) {
        self.code.push(0xE9);
        self.fixups.push((self.code.len() as u32, l.0, 0));
        self.imm32(0);
    }
    fn jcc(&mut self, c: Cond, l: Label) {
        self.jcc_raw(Self::cc(c), l);
    }
    fn branch_reg(&mut self, r: u8, nz: bool, l: Label) {
        self.op(&[], true, &[0x85], r, Rm::Reg(r), false);
        self.jcc_raw(if nz { 5 } else { 4 }, l);
    }

    fn mov(&mut self, d: u8, s: u8) {
        if d != s {
            self.op(&[], true, &[0x89], s, Rm::Reg(d), false);
        }
    }
    fn mov_imm(&mut self, d: u8, v: i64) {
        if (0..=i64::from(u32::MAX)).contains(&v) {
            if d >= 8 {
                self.code.push(0x41);
            }
            self.code.push(0xB8 | (d & 7));
            self.code.extend_from_slice(&(v as u32).to_le_bytes());
        } else if i32::try_from(v).is_ok() {
            self.op(&[], true, &[0xC7], 0, Rm::Reg(d), false);
            self.imm32(v as i32);
        } else {
            self.code.push(0x48 | (d >> 3));
            self.code.push(0xB8 | (d & 7));
            self.code.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn mov_reloc(&mut self, d: u8, v: i64, n: u32) {
        // movabs: REX.W, B8+r, then the eight bytes of the address
        self.code.push(0x48 | (d >> 3));
        self.code.push(0xB8 | (d & 7));
        self.sites.push(crate::jit_backend::trace::reloc::Site {
            at: self.code.len() as u32,
            n,
        });
        self.code.extend_from_slice(&v.to_le_bytes());
    }
    fn fmov(&mut self, d: u8, s: u8) {
        self.movaps(d, s);
    }
    fn bits_to_f(&mut self, d: u8, s: u8) {
        self.op(&[0x66], true, &[0x0F, 0x6E], d, Rm::Reg(s), false);
    }
    fn bits_to_i(&mut self, d: u8, s: u8) {
        self.op(&[0x66], true, &[0x0F, 0x7E], s, Rm::Reg(d), false);
    }
    fn load(&mut self, w: Width, d: u8, base: u8, off: i32) {
        let m = Rm::Mem(base, off);
        match w {
            Width::B1 => self.op(&[], false, &[0x0F, 0xB6], d, m, false),
            Width::B2 => self.op(&[], false, &[0x0F, 0xB7], d, m, false),
            Width::B4 => self.op(&[], false, &[0x8B], d, m, false),
            Width::B8 => self.op(&[], true, &[0x8B], d, m, false),
        }
    }
    fn store(&mut self, w: Width, s: u8, base: u8, off: i32) {
        let m = Rm::Mem(base, off);
        match w {
            Width::B1 => self.op(&[], false, &[0x88], s, m, true),
            Width::B2 => self.op(&[0x66], false, &[0x89], s, m, false),
            Width::B4 => self.op(&[], false, &[0x89], s, m, false),
            Width::B8 => self.op(&[], true, &[0x89], s, m, false),
        }
    }
    fn fload(&mut self, d: u8, base: u8, off: i32) {
        self.op(&[0xF2], false, &[0x0F, 0x10], d, Rm::Mem(base, off), false);
    }
    fn fstore(&mut self, s: u8, base: u8, off: i32) {
        self.op(&[0xF2], false, &[0x0F, 0x11], s, Rm::Mem(base, off), false);
    }
    fn lea(&mut self, d: u8, base: u8, off: i32) {
        self.op(&[], true, &[0x8D], d, Rm::Mem(base, off), false);
    }

    fn alu(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8) {
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
            Alu::Sdiv | Alu::Umulhi => {
                self.mov(RAX, a);
                // sign-extend rax into rdx for idiv; mul overwrites rdx
                if op == Alu::Sdiv {
                    if wide {
                        self.code.push(0x48);
                    }
                    self.code.push(0x99);
                }
                let (ext, r) = if op == Alu::Sdiv { (7, RAX) } else { (4, RDX) };
                self.op(&[], wide, &[0xF7], ext, Rm::Reg(b), false);
                if wide {
                    self.mov(d, r);
                } else {
                    self.mov32(d, r);
                }
            }
        }
    }
    fn alu_imm(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool {
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
            Alu::Sdiv | Alu::Umulhi => return false,
        }
        true
    }
    fn neg(&mut self, wide: bool, d: u8, a: u8) {
        if wide {
            self.mov(d, a)
        } else if d != a {
            self.mov32(d, a)
        }
        self.op(&[], wide, &[0xF7], 3, Rm::Reg(d), false);
    }
    fn not(&mut self, wide: bool, d: u8, a: u8) {
        if wide {
            self.mov(d, a)
        } else if d != a {
            self.mov32(d, a)
        }
        self.op(&[], wide, &[0xF7], 2, Rm::Reg(d), false);
    }
    fn cmp(&mut self, wide: bool, a: u8, b: u8) {
        self.op(&[], wide, &[0x39], b, Rm::Reg(a), false);
    }
    fn cmp_imm(&mut self, wide: bool, a: u8, imm: i64) -> bool {
        let imm = if wide { imm } else { i64::from(imm as i32) };
        let Ok(v) = i32::try_from(imm) else {
            return false;
        };
        self.grp1(wide, 7, a, v);
        true
    }
    fn setcc(&mut self, d: u8, c: Cond) {
        self.set8(Self::cc(c), d);
        self.movzx8(d, d);
    }
    fn csel(&mut self, d: u8, c: Cond, a: u8, b: u8) {
        let cc = Self::cc(c);
        if d == a {
            self.op(&[], true, &[0x0F, 0x40 | (cc ^ 1)], d, Rm::Reg(b), false);
        } else {
            self.mov(d, b);
            self.op(&[], true, &[0x0F, 0x40 | cc], d, Rm::Reg(a), false);
        }
    }
    fn zext(&mut self, d: u8, a: u8, bits: u32) {
        match bits {
            8 => self.movzx8(d, a),
            16 => self.op(&[], false, &[0x0F, 0xB7], d, Rm::Reg(a), false),
            32 => self.mov32(d, a),
            _ => self.mov(d, a),
        }
    }
    fn sext(&mut self, d: u8, a: u8, bits: u32) {
        match bits {
            8 => self.op(&[], true, &[0x0F, 0xBE], d, Rm::Reg(a), true),
            16 => self.op(&[], true, &[0x0F, 0xBF], d, Rm::Reg(a), false),
            32 => self.op(&[], true, &[0x63], d, Rm::Reg(a), false),
            _ => self.mov(d, a),
        }
    }

    fn fbin(&mut self, op: BinOp, d: u8, a: u8, b: u8) {
        let opc = match op {
            BinOp::Fadd => 0x58,
            BinOp::Fmul => 0x59,
            BinOp::Fsub => 0x5C,
            _ => 0x5E,
        };
        let commutes = matches!(op, BinOp::Fadd | BinOp::Fmul);
        if d == b && d != a {
            if commutes {
                self.sse(0xF2, opc, d, a);
            } else {
                self.movaps(XTMP, a);
                self.sse(0xF2, opc, XTMP, b);
                self.movaps(d, XTMP);
            }
            return;
        }
        self.movaps(d, a);
        self.sse(0xF2, opc, d, b);
    }
    fn fneg(&mut self, d: u8, a: u8) {
        self.mov_imm(RAX, i64::MIN);
        self.bits_to_f(XTMP, RAX);
        self.movaps(d, a);
        self.op(&[0x66], false, &[0x0F, 0x57], d, Rm::Reg(XTMP), false);
    }
    fn fround(&mut self, d: u8, a: u8, up: bool) -> bool {
        if !std::is_x86_feature_detected!("sse4.1") {
            return false;
        }
        self.op(&[0x66], false, &[0x0F, 0x3A, 0x0B], d, Rm::Reg(a), false);
        self.code.push(if up { 0x0A } else { 0x09 });
        true
    }
    fn i2f(&mut self, d: u8, a: u8) {
        self.op(&[0x66], false, &[0x0F, 0x57], d, Rm::Reg(d), false);
        self.op(&[0xF2], true, &[0x0F, 0x2A], d, Rm::Reg(a), false);
    }
    fn f2i(&mut self, d: u8, a: u8) {
        self.op(&[0xF2], true, &[0x0F, 0x2C], d, Rm::Reg(a), false);
    }
    fn f2i_sat(&mut self, d: u8, a: u8) {
        let (done, nan) = (self.new_label(), self.new_label());
        self.f2i(d, a);
        // only i64::MIN overflows on `cmp d, 1`
        self.grp1(true, 7, d, 1);
        self.jcc_raw(1, done);
        self.ucomisd(a, a);
        self.jcc_raw(10, nan);
        self.op(&[0x66], false, &[0x0F, 0x57], XTMP, Rm::Reg(XTMP), false);
        self.ucomisd(a, XTMP);
        self.jcc_raw(2, done);
        self.mov_imm(d, i64::MAX);
        self.jmp(done);
        self.bind(nan);
        self.mov_imm(d, 0);
        self.bind(done);
    }
    fn fcmp_set(&mut self, d: u8, cc: FloatCC, a: u8, b: u8) -> bool {
        match cc {
            FloatCC::Equal | FloatCC::NotEqual => {
                self.ucomisd(a, b);
                let eq = cc == FloatCC::Equal;
                self.set8(if eq { 4 } else { 5 }, d);
                self.set8(if eq { 11 } else { 10 }, RAX);
                let opc = if eq { 0x20 } else { 0x08 };
                self.op(&[], false, &[opc], RAX, Rm::Reg(d), true);
            }
            FloatCC::LessThan | FloatCC::LessThanOrEqual => {
                self.ucomisd(b, a);
                self.set8(if cc == FloatCC::LessThan { 7 } else { 3 }, d);
            }
            FloatCC::GreaterThan | FloatCC::GreaterThanOrEqual => {
                self.ucomisd(a, b);
                self.set8(if cc == FloatCC::GreaterThan { 7 } else { 3 }, d);
            }
            _ => return false,
        }
        self.movzx8(d, d);
        true
    }

    fn call_abs(&mut self, addr: usize) {
        self.mov_imm(R11, addr as i64);
        self.call_reg(R11);
    }
    fn call_reg(&mut self, r: u8) {
        self.op(&[], false, &[0xFF], 2, Rm::Reg(r), false);
    }
    fn prologue(&mut self, saved: &[u8], fsaved: &[u8], locals: u32) {
        assert!(
            fsaved.is_empty(),
            "no callee-saved float register is allocated"
        );
        self.saved = saved.to_vec();
        for &r in saved {
            if r >= 8 {
                self.code.push(0x41);
            }
            self.code.push(0x50 | (r & 7));
        }
        // the return address and the pushes leave rsp 8 + 8k mod 16
        let pushed = 8 + 8 * saved.len() as u32;
        self.frame = locals + (16 - pushed % 16) % 16;
        if self.frame > 0 {
            self.op(&[], true, &[0x81], 5, Rm::Reg(RSP), false);
            self.imm32(self.frame as i32);
        }
    }
    fn epilogue_ret(&mut self) {
        if self.frame > 0 {
            self.op(&[], true, &[0x81], 0, Rm::Reg(RSP), false);
            self.imm32(self.frame as i32);
        }
        for k in (0..self.saved.len()).rev() {
            let r = self.saved[k];
            if r >= 8 {
                self.code.push(0x41);
            }
            self.code.push(0x58 | (r & 7));
        }
        self.code.push(0xC3);
    }
    fn new(b: Bufs) -> X64 {
        let Bufs {
            bytes: mut code,
            words,
            mut labels,
            mut fixups,
            mut sites,
        } = b;
        code.clear();
        labels.clear();
        fixups.clear();
        sites.clear();
        X64 {
            code,
            labels,
            fixups,
            words,
            sites,
            saved: Vec::new(),
            frame: 0,
        }
    }
    fn finish(mut self) -> Bufs {
        for &(at, l, _) in &self.fixups {
            let target = self.labels[l as usize] as i64;
            let rel = (target - (i64::from(at) + 4)) as i32;
            self.code[at as usize..at as usize + 4].copy_from_slice(&rel.to_le_bytes());
        }
        Bufs {
            bytes: self.code,
            words: self.words,
            labels: self.labels,
            fixups: self.fixups,
            sites: self.sites,
        }
    }
}
