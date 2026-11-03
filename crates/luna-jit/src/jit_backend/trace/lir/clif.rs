//! A baseline trace's instructions replayed as Cranelift IR: the optimizing
//! tier compiles exactly what the baseline tier ran, with the same cells,
//! exits and helpers, so the two entries are interchangeable.

use super::*;
use cranelift_codegen::ir::immediates::Ieee64;
use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, InstBuilder, MemFlagsData, Signature, Type, UserFuncName, Value,
    types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use live::Start;

mod define;
mod entry;
mod ops;
pub(crate) use define::define;
#[cfg(feature = "llvm-jit")]
pub(crate) use entry::{dominates, idoms, live_in_vars};
use ops::hoist;

fn cty(t: Ty) -> Type {
    match t {
        Ty::I8 => types::I8,
        Ty::I16 => types::I16,
        Ty::I32 => types::I32,
        Ty::I64 => types::I64,
        Ty::F64 => types::F64,
    }
}

fn sig_of(lir: &Lir, c: &Callee, call_conv: CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    for &t in lir.params(c) {
        sig.params.push(AbiParam::new(cty(t)));
    }
    if let Some(t) = c.ret {
        sig.returns.push(AbiParam::new(cty(t)));
    }
    sig
}

struct Replay<'a, 'f> {
    lir: &'a Lir,
    b: FunctionBuilder<'f>,
    vals: Vec<Option<Value>>,
    vars: Vec<Variable>,
    blocks: Vec<Option<Block>>,
    slots: Vec<cranelift_codegen::ir::StackSlot>,
    call_conv: CallConv,
    /// The data symbol each relocation is read from.
    reloc_gv: Vec<cranelift_codegen::ir::GlobalValue>,
    /// Relocations read once, in the entry block, for every use: the
    /// trace module is compiled without optimization, which would leave
    /// a string key or a function checked against read anew at each use
    /// inside the loop ([`HOISTED`]).
    hoisted: Vec<Option<Value>>,
    start: live::Start,
    /// In the loop's function, per value the loop head takes from the back
    /// edges (see `LoopEntry::inherit`): the variable it was taken from, and
    /// the block the replay defined it in (`NONE` before).
    ivars: Vec<Option<(Variable, u32)>>,
    /// Those values as the replay defined them.
    defs: Vec<Option<Value>>,
    /// Per block, its immediate dominator (the loop's function only).
    idom: Vec<u32>,
}

/// The most relocations read in the entry block ([`ops::hoist`]).
const HOISTED: usize = 4;

impl Replay<'_, '_> {
    fn v(&self, n: u32) -> Value {
        self.vals[n as usize].expect("a value is defined before its uses in layout order")
    }

    fn args(&self, i: &Inst) -> Vec<Value> {
        self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize]
            .iter()
            .map(|&a| self.v(a))
            .collect()
    }

    fn block_args(&self, args: &[Value]) -> Vec<BlockArg> {
        args.iter().map(|&a| BlockArg::Value(a)).collect()
    }

    /// The immediate operand of `i`: the lowerer's constant when it is
    /// defined here (one register for every use, as in the lowerer's own
    /// Cranelift IR), else a new one.
    fn imm(&mut self, i: &Inst, t: Type, imm: i64) -> Value {
        match self.vals.get(i.c as usize).copied().flatten() {
            Some(c) => c,
            None => self.b.ins().iconst(t, imm),
        }
    }

    fn set(&mut self, dst: u32, v: Value) {
        self.vals[dst as usize] = Some(v);
    }

    fn inst(&mut self, i: &Inst) {
        let t = cty(i.ty);
        match i.op {
            Op::Iconst(n) => {
                let v = self.b.ins().iconst(t, n);
                self.set(i.dst, v);
            }
            Op::Reloc(n) => {
                let v = match self.hoisted[n as usize] {
                    Some(v) => v,
                    None => self
                        .b
                        .ins()
                        .symbol_value(types::I64, self.reloc_gv[n as usize]),
                };
                self.set(i.dst, v);
            }
            Op::Fconst(bits) => {
                let v = self.b.ins().f64const(Ieee64::with_bits(bits));
                self.set(i.dst, v);
            }
            Op::Bin(op) => {
                let v = self.bin(op, self.v(i.a), self.v(i.b));
                self.set(i.dst, v);
            }
            Op::BinImm(op, imm) => {
                let c = self.imm(i, t, imm);
                let v = self.bin(op, self.v(i.a), c);
                self.set(i.dst, v);
            }
            Op::Un(u) => {
                let v = self.un(u, i);
                self.set(i.dst, v);
            }
            Op::Icmp(cc) => {
                let o0 = self.v(i.a);
                let o1 = self.v(i.b);
                let v = self.b.ins().icmp(cc, o0, o1);
                self.set(i.dst, v);
            }
            Op::IcmpImm(cc, imm) => {
                let c = self.imm(i, t, imm);
                let o0 = self.v(i.a);
                let v = self.b.ins().icmp(cc, o0, c);
                self.set(i.dst, v);
            }
            Op::Fcmp(cc) => {
                let o0 = self.v(i.a);
                let o1 = self.v(i.b);
                let v = self.b.ins().fcmp(cc, o0, o1);
                self.set(i.dst, v);
            }
            Op::Select => {
                let o0 = self.v(i.a);
                let o1 = self.v(i.b);
                let o2 = self.v(i.c);
                let v = self.b.ins().select(o0, o1, o2);
                self.set(i.dst, v);
            }
            Op::Load(off) => {
                let o0 = self.v(i.a);
                let v = self.b.ins().load(t, flags(i.c), o0, off);
                self.set(i.dst, v);
            }
            Op::Uload8(off) => {
                let o0 = self.v(i.a);
                let v = self.b.ins().uload8(t, flags(i.c), o0, off);
                self.set(i.dst, v);
            }
            Op::Store(off) => {
                let o0 = self.v(i.a);
                let o1 = self.v(i.b);
                self.b.ins().store(flags(i.c), o0, o1, off);
            }
            Op::StackAddr(slot, off) => {
                let v = self
                    .b
                    .ins()
                    .stack_addr(types::I64, self.slots[slot as usize], off);
                self.set(i.dst, v);
            }
            Op::StackLoad(slot, off) => {
                let v = self
                    .b
                    .ins()
                    .stack_load(types::I64, t, self.slots[slot as usize], off);
                self.set(i.dst, v);
            }
            Op::StackStore(slot, off) => {
                let o0 = self.v(i.a);
                self.b
                    .ins()
                    .stack_store(types::I64, o0, self.slots[slot as usize], off);
            }
            Op::Jump
            | Op::Brif(_)
            | Op::TierCount { .. }
            | Op::Call
            | Op::CallIndirect
            | Op::Return => self.control(i),
            Op::VarRead => {
                let v = self.b.use_var(self.vars[i.a as usize]);
                self.set(i.dst, v);
            }
            Op::VarWrite => {
                let v = self.v(i.b);
                self.b.def_var(self.vars[i.a as usize], v);
            }
        }
    }
}

/// The flags the lowerer gave a memory access (see `Op::Load`).
fn flags(trusted: u32) -> MemFlagsData {
    if trusted == 1 {
        MemFlagsData::trusted()
    } else {
        MemFlagsData::new()
    }
}
