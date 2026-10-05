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
    /// Whether the code counts iterations like the baseline tier's.
    count: bool,
}

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

    fn bin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        let ins = self.b.ins();
        match op {
            BinOp::Add => ins.iadd(x, y),
            BinOp::Sub => ins.isub(x, y),
            BinOp::Mul => ins.imul(x, y),
            BinOp::Sdiv => ins.sdiv(x, y),
            BinOp::Umulhi => ins.umulhi(x, y),
            BinOp::Smin => ins.smin(x, y),
            BinOp::Smax => ins.smax(x, y),
            BinOp::And => ins.band(x, y),
            BinOp::Or => ins.bor(x, y),
            BinOp::Xor => ins.bxor(x, y),
            BinOp::Shl => ins.ishl(x, y),
            BinOp::Ushr => ins.ushr(x, y),
            BinOp::Sshr => ins.sshr(x, y),
            BinOp::Fadd => ins.fadd(x, y),
            BinOp::Fsub => ins.fsub(x, y),
            BinOp::Fmul => ins.fmul(x, y),
            BinOp::Fdiv => ins.fdiv(x, y),
        }
    }

    fn un(&mut self, u: UnOp, i: &Inst) -> Value {
        let x = self.v(i.a);
        let from = self.lir.value_ty[i.a as usize];
        let t = cty(i.ty);
        let ins = self.b.ins();
        match u {
            UnOp::Ineg => ins.ineg(x),
            UnOp::Bnot => ins.bnot(x),
            UnOp::Fneg => ins.fneg(x),
            UnOp::Floor => ins.floor(x),
            UnOp::Ceil => ins.ceil(x),
            UnOp::Uextend if from == i.ty => x,
            UnOp::Uextend => ins.uextend(t, x),
            UnOp::Ireduce if from == i.ty => x,
            UnOp::Ireduce => ins.ireduce(t, x),
            UnOp::Bitcast => ins.bitcast(t, MemFlagsData::new(), x),
            UnOp::FcvtFromSint => ins.fcvt_from_sint(t, x),
            UnOp::FcvtToSint => ins.fcvt_to_sint(t, x),
            UnOp::FcvtToSintSat => ins.fcvt_to_sint_sat(t, x),
        }
    }

    fn inst(&mut self, i: &Inst) {
        let lir = self.lir;
        let t = cty(i.ty);
        match i.op {
            Op::Iconst(n) => {
                let v = self.b.ins().iconst(t, n);
                self.set(i.dst, v);
            }
            Op::Reloc(n) => {
                let v = self
                    .b
                    .ins()
                    .symbol_value(types::I64, self.reloc_gv[n as usize]);
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
            Op::Jump => {
                let a = self.args(i);
                let target = self.blocks[i.a as usize].expect("reachable");
                let ba = self.block_args(&a);
                self.b.ins().jump(target, &ba);
            }
            Op::Brif(n_then) => {
                let a = self.args(i);
                let (ta, ea) = a.split_at(n_then as usize);
                let (tb, eb) = (
                    self.blocks[i.b as usize].expect("reachable"),
                    self.blocks[i.c as usize].expect("reachable"),
                );
                let (ta, ea) = (self.block_args(ta), self.block_args(ea));
                let o0 = self.v(i.a);
                self.b.ins().brif(o0, tb, &ta, eb, &ea);
            }
            Op::TierCount { n, at } if self.count => {
                let cell = self
                    .b
                    .ins()
                    .symbol_value(types::I64, self.reloc_gv[n as usize]);
                let c = self
                    .b
                    .ins()
                    .load(types::I32, MemFlagsData::trusted(), cell, 0);
                let c = self.b.ins().iadd_imm_u(c, 1);
                self.b.ins().store(MemFlagsData::trusted(), c, cell, 0);
                let hot = self.b.ins().icmp_imm_u(IntCC::Equal, c, i64::from(at));
                let (h, k) = (
                    self.blocks[i.b as usize].expect("reachable"),
                    self.blocks[i.c as usize].expect("reachable"),
                );
                self.b.ins().brif(hot, h, &[], k, &[]);
            }
            // otherwise only the baseline tier counts iterations
            Op::TierCount { .. } => {
                let cont = self.blocks[i.c as usize].expect("reachable");
                self.b.ins().jump(cont, &[]);
            }
            Op::Call | Op::CallIndirect => {
                let (callee, target) = match i.op {
                    Op::Call => {
                        let f = &lir.funcs[i.a as usize];
                        (f, self.b.ins().iconst(types::I64, f.addr as i64))
                    }
                    _ => (&lir.sigs[i.b as usize], self.v(i.a)),
                };
                let sig = sig_of(lir, callee, self.call_conv);
                let sref = self.b.import_signature(sig);
                let a = self.args(i);
                let call = self.b.ins().call_indirect(sref, target, &a);
                if i.dst != NONE {
                    let r = self.b.inst_results(call)[0];
                    self.set(i.dst, r);
                }
            }
            Op::Return => {
                let r: Vec<Value> = (i.a != NONE).then(|| self.v(i.a)).into_iter().collect();
                self.b.ins().return_(&r);
            }
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

/// The blocks the replay of `i` branches to (of a `TierCount` only the
/// continuation, unless the code counts).
fn targets(i: &Inst, count: bool) -> [Option<u32>; 2] {
    match i.op {
        Op::Jump => [Some(i.a), None],
        Op::Brif(_) => [Some(i.b), Some(i.c)],
        Op::TierCount { .. } => [Some(i.c), count.then_some(i.b)],
        _ => [None, None],
    }
}

/// Defines the trace function in `module` from `lir`.
/// `relocs`: the addresses the relocations stand for in the Vm the code is
/// for. `count`: the code counts loop iterations as the baseline tier's
/// does and leaves at the same count, for a tier after this one.
pub(crate) fn define<M: Module>(
    lir: &Lir,
    relocs: &[(super::super::RelocKind, i64)],
    module: &mut M,
    count: bool,
) -> Option<FuncId> {
    let mut an = live::Analysis::default();
    live::analyze(lir, &mut an);
    let call_conv = module.isa().default_call_conv();
    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let fn_id = module
        .declare_function("luna_jit_trace", Linkage::Local, &sig)
        .ok()?;
    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let mut blocks = vec![None; lir.blocks.len()];
    for &blk in &an.order {
        blocks[blk as usize] = Some(b.create_block());
    }
    let mut vals = vec![None; lir.value_ty.len()];
    let entry = blocks[0].expect("the entry block");
    b.append_block_params_for_function_params(entry);
    if lir.arg0 != NONE {
        vals[lir.arg0 as usize] = Some(b.block_params(entry)[0]);
    }
    for &blk in &an.order {
        if blk == 0 {
            continue;
        }
        let cb = blocks[blk as usize].expect("laid out");
        for &p in lir.block_params(blk) {
            vals[p as usize] = Some(b.append_block_param(cb, cty(lir.value_ty[p as usize])));
        }
    }
    let vars = lir.var_ty.iter().map(|&t| b.declare_var(cty(t))).collect();
    let slots = lir
        .slots
        .iter()
        .map(|&(size, align)| {
            b.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                size,
                align,
            ))
        })
        .collect();
    let mut reloc_gv = Vec::with_capacity(relocs.len());
    for n in 0..relocs.len() {
        let id = module
            .declare_data(
                &crate::jit_backend::trace::reloc_symbol(n),
                Linkage::Import,
                false,
                false,
            )
            .ok()?;
        reloc_gv.push(module.declare_data_in_func(id, b.func));
    }
    let mut r = Replay {
        lir,
        b,
        vals,
        vars,
        blocks,
        slots,
        call_conv,
        reloc_gv,
        count,
    };
    // each block is sealed once its last predecessor branches to it: with
    // every block left open until the end, Cranelift's SSA construction
    // gives loop heads and merges a parameter for each variable read there,
    // and the loop carries all of them
    let mut preds = vec![0u32; lir.blocks.len()];
    let mut sealed = vec![false; lir.blocks.len()];
    for &blk in &an.order {
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            for t in targets(&lir.insts[an.code[c as usize] as usize], count)
                .into_iter()
                .flatten()
            {
                preds[t as usize] += 1;
            }
        }
    }
    for &blk in &an.order {
        let cb = r.blocks[blk as usize].expect("laid out");
        r.b.switch_to_block(cb);
        // a block no branch reaches (the entry, or the hot exit of a
        // `TierCount` when the replay does not count) is sealed as it starts
        if preds[blk as usize] == 0 && !sealed[blk as usize] {
            r.b.seal_block(cb);
            sealed[blk as usize] = true;
        }
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            let i = lir.insts[an.code[c as usize] as usize];
            r.inst(&i);
            for t in targets(&i, count).into_iter().flatten() {
                preds[t as usize] -= 1;
                if preds[t as usize] == 0 {
                    r.b.seal_block(r.blocks[t as usize].expect("laid out"));
                    sealed[t as usize] = true;
                }
            }
        }
    }
    r.b.finalize(module.target_config());
    crate::jit_backend::trace::drop_unused_block_params(&mut ctx.func);
    // `LUNA_TRACE_IR_DUMP=1`, as for the lowerer's own Cranelift IR
    if std::env::var_os("LUNA_TRACE_IR_DUMP").is_some_and(|v| v == "1") {
        eprintln!(
            "=== TRACE IR DUMP (from baseline) ===\n{}\n=== END ===",
            ctx.func.display()
        );
    }
    // and `LUNA_TRACE_ASM_DUMP=1` for the machine code
    let asm_dump = std::env::var_os("LUNA_TRACE_ASM_DUMP").is_some_and(|v| v == "1");
    if asm_dump {
        ctx.set_disasm(true);
    }
    crate::jit_backend::trace::reloc::set_values(relocs);
    module.define_function(fn_id, &mut ctx).ok()?;
    crate::jit_backend::trace::code_dump::note_size(&ctx);
    crate::jit_backend::trace::reloc::note_sites(&*module, &ctx);
    if asm_dump && let Some(vcode) = ctx.compiled_code().and_then(|c| c.vcode.as_ref()) {
        eprintln!("=== TRACE ASM DUMP (from baseline) ===\n{vcode}\n=== END ===");
    }
    module.clear_context(&mut ctx);
    Some(fn_id)
}
