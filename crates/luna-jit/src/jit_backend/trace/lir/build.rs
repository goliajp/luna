//! Recording the lowerer's instructions into a [`Lir`].
//!
//! Variables are not put in SSA form. Each variable has a home the backend
//! allocates like any value: `def_var` writes it, and `use_var` reads it
//! unless the variable's value is already known in the current block, either
//! from a write in the block or inherited from the block's only predecessor.
//! Reusing the known value keeps the lowerer's "register unchanged since the
//! last store" comparisons working, which is what keeps exits small.

use super::record::{mask, v, val};
use super::ty::trusted;
use super::*;
use crate::jit_backend::trace::Ins;
use cranelift_codegen::ir::condcodes::CondCode;
use cranelift_codegen::ir::immediates::{Ieee64, Imm64, Offset32};
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, Inst as CInst, MemFlagsData, SigRef, StackSlot, StackSlotData, Type,
    Value,
};
use cranelift_frontend::Variable;

impl Ins for Lir {
    fn create_block(&mut self) -> Block {
        self.new_block()
    }
    fn switch_to_block(&mut self, b: Block) {
        self.enter_block(b)
    }
    fn seal_block(&mut self, b: Block) {
        self.seal(b)
    }
    fn set_cold_block(&mut self, b: Block) {
        self.blocks[b.as_u32() as usize].cold = true;
    }
    fn mark_reentry(&mut self, b: Block) {
        self.blocks[b.as_u32() as usize].reentry = true;
    }
    fn mark_relink(&mut self, b: Block, head_pc: u32, ran_off: i32) {
        let block = b.as_u32();
        self.relink = Some(Relink {
            block,
            head_pc,
            ran_off,
        });
    }
    fn current_block(&self) -> Option<Block> {
        (self.cur != NONE).then(|| Block::from_u32(self.cur))
    }
    fn append_block_param(&mut self, b: Block, ty: Type) -> Value {
        let d = self.new_value(Ty::of(ty));
        self.add_block_param(b.as_u32(), d);
        val(d)
    }
    fn append_block_params_for_function_params(&mut self, b: Block) {
        let d = self.new_value(Ty::I64);
        self.arg0 = d;
        self.add_block_param(b.as_u32(), d);
    }
    fn block_params(&self, b: Block) -> &[Value] {
        let blk = &self.blocks[b.as_u32() as usize];
        &self.bparam_vals[blk.params_at as usize..(blk.params_at + blk.n_params) as usize]
    }
    fn declare_var(&mut self, ty: Type) -> Variable {
        self.var_ty.push(Ty::of(ty));
        self.var_cur.push(NONE);
        Variable::from_u32(self.var_ty.len() as u32 - 1)
    }
    fn def_var(&mut self, var: Variable, x: Value) {
        let n = var.as_u32();
        self.push(Op::VarWrite, self.var_ty[n as usize], NONE, n, v(x), NONE);
        self.var_cur[n as usize] = v(x);
    }
    #[inline]
    fn use_var(&mut self, var: Variable) -> Value {
        let n = var.as_u32();
        let known = self.var_cur[n as usize];
        if known != NONE {
            return val(known);
        }
        self.read_var(n)
    }
    fn create_sized_stack_slot(&mut self, data: StackSlotData) -> StackSlot {
        self.slots.push((data.size, data.align_shift));
        StackSlot::from_u32(self.slots.len() as u32 - 1)
    }
    fn inst_results(&self, inst: CInst) -> &[Value] {
        let i = &self.insts[inst.as_u32() as usize];
        if i.dst == NONE {
            return &[];
        }
        std::slice::from_ref(&i.res)
    }
    fn resolve_aliases(&self, x: Value) -> Value {
        x
    }

    fn iconst(&mut self, ty: Type, n: impl Into<Imm64>) -> Value {
        let t = Ty::of(ty);
        let n = mask(t, n.into().bits());
        let x = self.def(Op::Iconst(n), t, NONE, NONE, NONE);
        self.konst[v(x) as usize] = Some(n);
        x
    }
    fn f64const(&mut self, n: impl Into<Ieee64>) -> Value {
        self.def(Op::Fconst(n.into().bits()), Ty::F64, NONE, NONE, NONE)
    }
    fn iadd(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Add, x, y)
    }
    fn isub(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Sub, x, y)
    }
    fn imul(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Mul, x, y)
    }
    fn sdiv(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Sdiv, x, y)
    }
    fn udiv(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Udiv, x, y)
    }
    fn umulhi(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Umulhi, x, y)
    }
    fn ineg(&mut self, x: Value) -> Value {
        let t = self.ty_of_value(x);
        self.un(UnOp::Ineg, t, x)
    }
    fn smin(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Smin, x, y)
    }
    fn smax(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Smax, x, y)
    }
    fn band(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::And, x, y)
    }
    fn bor(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Or, x, y)
    }
    fn bxor(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Xor, x, y)
    }
    fn bnot(&mut self, x: Value) -> Value {
        let t = self.ty_of_value(x);
        self.un(UnOp::Bnot, t, x)
    }
    fn ishl(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Shl, x, y)
    }
    fn ushr(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Ushr, x, y)
    }
    fn iadd_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Add, x, y)
    }
    fn iadd_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Add, x, y)
    }
    fn band_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::And, x, y)
    }
    fn band_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::And, x, y)
    }
    fn bxor_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Xor, x, y)
    }
    fn ishl_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Shl, x, y)
    }
    fn ushr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Ushr, x, y)
    }
    fn sshr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value {
        self.bin_imm(BinOp::Sshr, x, y)
    }
    fn icmp(&mut self, cc: impl Into<IntCC>, x: Value, y: Value) -> Value {
        let cc = cc.into();
        let t = self.ty_of_value(x);
        let d = self.new_value(Ty::I8);
        if let Some(c) = self.konst[v(y) as usize] {
            self.push(Op::IcmpImm(cc, c), t, d, v(x), NONE, v(y));
        } else if let Some(c) = self.konst[v(x) as usize] {
            self.push(Op::IcmpImm(cc.swap_args(), c), t, d, v(y), NONE, v(x));
        } else {
            self.push(Op::Icmp(cc), t, d, v(x), v(y), NONE);
        }
        val(d)
    }
    fn icmp_imm_u(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value {
        let t = self.ty_of_value(x);
        let d = self.new_value(Ty::I8);
        self.push(Op::IcmpImm(cc, y.into().bits()), t, d, v(x), NONE, NONE);
        val(d)
    }
    fn icmp_imm_s(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value {
        self.icmp_imm_u(cc, x, y)
    }
    fn select(&mut self, c: Value, x: Value, y: Value) -> Value {
        let t = self.ty_of_value(x);
        self.def(Op::Select, t, v(c), v(x), v(y))
    }
    fn uextend(&mut self, ty: Type, x: Value) -> Value {
        self.un(UnOp::Uextend, Ty::of(ty), x)
    }
    fn ireduce(&mut self, ty: Type, x: Value) -> Value {
        self.un(UnOp::Ireduce, Ty::of(ty), x)
    }
    fn bitcast(&mut self, ty: Type, _flags: impl Into<MemFlagsData>, x: Value) -> Value {
        self.un(UnOp::Bitcast, Ty::of(ty), x)
    }
    fn fadd(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Fadd, x, y)
    }
    fn fsub(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Fsub, x, y)
    }
    fn fmul(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Fmul, x, y)
    }
    fn fdiv(&mut self, x: Value, y: Value) -> Value {
        self.bin(BinOp::Fdiv, x, y)
    }
    fn fneg(&mut self, x: Value) -> Value {
        self.un(UnOp::Fneg, Ty::F64, x)
    }
    fn floor(&mut self, x: Value) -> Value {
        self.un(UnOp::Floor, Ty::F64, x)
    }
    fn ceil(&mut self, x: Value) -> Value {
        self.un(UnOp::Ceil, Ty::F64, x)
    }
    fn fcmp(&mut self, cc: impl Into<FloatCC>, x: Value, y: Value) -> Value {
        let d = self.new_value(Ty::I8);
        self.push(Op::Fcmp(cc.into()), Ty::F64, d, v(x), v(y), NONE);
        val(d)
    }
    fn fcvt_from_sint(&mut self, ty: Type, x: Value) -> Value {
        self.un(UnOp::FcvtFromSint, Ty::of(ty), x)
    }
    fn fcvt_to_sint(&mut self, ty: Type, x: Value) -> Value {
        self.un(UnOp::FcvtToSint, Ty::of(ty), x)
    }
    fn fcvt_to_sint_sat(&mut self, ty: Type, x: Value) -> Value {
        self.un(UnOp::FcvtToSintSat, Ty::of(ty), x)
    }
    fn load(
        &mut self,
        ty: Type,
        flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value {
        let op = Op::Load(off.into().into());
        self.def(op, Ty::of(ty), v(p), NONE, trusted(flags))
    }
    fn uload8(
        &mut self,
        ty: Type,
        flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value {
        let op = Op::Uload8(off.into().into());
        self.def(op, Ty::of(ty), v(p), NONE, trusted(flags))
    }
    fn store(
        &mut self,
        flags: impl Into<MemFlagsData>,
        x: Value,
        p: Value,
        off: impl Into<Offset32>,
    ) -> CInst {
        let t = self.ty_of_value(x);
        let op = Op::Store(off.into().into());
        CInst::from_u32(self.push(op, t, NONE, v(x), v(p), trusted(flags)))
    }
    fn stack_addr(&mut self, _ty: Type, ss: StackSlot, off: impl Into<Offset32>) -> Value {
        self.def(
            Op::StackAddr(ss.as_u32(), off.into().into()),
            Ty::I64,
            NONE,
            NONE,
            NONE,
        )
    }
    fn stack_load(
        &mut self,
        _ptr_ty: Type,
        ty: Type,
        ss: StackSlot,
        off: impl Into<Offset32>,
    ) -> Value {
        self.def(
            Op::StackLoad(ss.as_u32(), off.into().into()),
            Ty::of(ty),
            NONE,
            NONE,
            NONE,
        )
    }
    fn stack_store(
        &mut self,
        _ptr_ty: Type,
        x: Value,
        ss: StackSlot,
        off: impl Into<Offset32>,
    ) -> CInst {
        let t = self.ty_of_value(x);
        let op = Op::StackStore(ss.as_u32(), off.into().into());
        CInst::from_u32(self.push(op, t, NONE, v(x), NONE, NONE))
    }
    fn jump<'a>(&mut self, b: Block, args: impl IntoIterator<Item = &'a BlockArg>) -> CInst {
        self.jump_to(b, args)
    }
    fn brif<'a>(
        &mut self,
        c: Value,
        then_b: Block,
        then_args: impl IntoIterator<Item = &'a BlockArg>,
        else_b: Block,
        else_args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> CInst {
        self.branch(c, then_b, then_args, else_b, else_args)
    }
    fn call(&mut self, f: FuncRef, args: &[Value]) -> CInst {
        let n = f.as_u32();
        let ret = self.funcs[n as usize].ret;
        let i = self.push(Op::Call, Ty::I64, NONE, n, NONE, NONE);
        self.calls = true;
        self.with_args(i, args.iter().map(|x| v(*x)));
        self.call_result(i, ret);
        CInst::from_u32(i)
    }
    fn call_indirect(&mut self, sig: SigRef, callee: Value, args: &[Value]) -> CInst {
        let ret = self.sigs[sig.as_u32() as usize].ret;
        self.calls = true;
        let i = self.push(
            Op::CallIndirect,
            Ty::I64,
            NONE,
            v(callee),
            sig.as_u32(),
            NONE,
        );
        self.with_args(i, args.iter().map(|x| v(*x)));
        self.call_result(i, ret);
        CInst::from_u32(i)
    }
    fn return_(&mut self, rvals: &[Value]) -> CInst {
        let r = rvals.first().map_or(NONE, |x| v(*x));
        CInst::from_u32(self.push(Op::Return, Ty::I64, NONE, r, NONE, NONE))
    }
}
