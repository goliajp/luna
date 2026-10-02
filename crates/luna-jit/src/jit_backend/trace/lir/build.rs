//! Recording the lowerer's instructions into a [`Lir`].
//!
//! Variables are not put in SSA form. Each variable has a home the backend
//! allocates like any value: `def_var` writes it, and `use_var` reads it
//! unless the variable's value is already known in the current block, either
//! from a write in the block or inherited from the block's only predecessor.
//! Reusing the known value keeps the lowerer's "register unchanged since the
//! last store" comparisons working, which is what keeps exits small.

use super::*;
use crate::jit_backend::trace::{Emit, Ins};
use cranelift_codegen::entity::packed_option::ReservedValue;
use cranelift_codegen::ir::immediates::{Ieee64, Imm64, Offset32};
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, GlobalValue, Inst as CInst, MemFlagsData, SigRef, Signature,
    StackSlot, StackSlotData, Type, Value,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::Variable;
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, ModuleError, ModuleResult};

fn v(x: Value) -> u32 {
    x.as_u32()
}

fn val(n: u32) -> Value {
    Value::from_u32(n)
}

fn mask(ty: Ty, n: i64) -> i64 {
    match ty {
        Ty::I8 => n & 0xff,
        Ty::I16 => n & 0xffff,
        Ty::I32 => n & 0xffff_ffff,
        Ty::I64 | Ty::F64 => n,
    }
}

impl Lir {
    pub(crate) fn new() -> Lir {
        Lir {
            arg0: NONE,
            cur: NONE,
            ..Lir::default()
        }
    }

    fn new_value(&mut self, ty: Ty) -> u32 {
        self.value_ty.push(ty);
        (self.value_ty.len() - 1) as u32
    }

    fn push(&mut self, op: Op, ty: Ty, dst: u32, a: u32, b: u32, c: u32) -> u32 {
        let idx = self.insts.len() as u32;
        self.insts.push(Inst {
            op,
            ty,
            dst,
            a,
            b,
            c,
            args_at: 0,
            n_args: 0,
            next: NONE,
            res: if dst == NONE {
                Value::reserved_value()
            } else {
                val(dst)
            },
        });
        let blk = &mut self.blocks[self.cur as usize];
        if blk.first == NONE {
            blk.first = idx;
        } else {
            let last = blk.last as usize;
            self.insts[last].next = idx;
        }
        self.blocks[self.cur as usize].last = idx;
        idx
    }

    fn def(&mut self, op: Op, ty: Ty, a: u32, b: u32, c: u32) -> Value {
        let d = self.new_value(ty);
        self.push(op, ty, d, a, b, c);
        val(d)
    }

    fn with_args(&mut self, inst: u32, args: impl IntoIterator<Item = u32>) {
        let at = self.args.len() as u32;
        self.args.extend(args);
        let i = &mut self.insts[inst as usize];
        i.args_at = at;
        i.n_args = self.args.len() as u32 - at;
    }

    fn push_block_args<'a>(&mut self, args: impl IntoIterator<Item = &'a BlockArg>) -> u32 {
        let at = self.args.len();
        for a in args {
            let x = match a {
                BlockArg::Value(x) => v(*x),
                _ => {
                    self.unsupported = Some("block argument other than a value");
                    0
                }
            };
            self.args.push(x);
        }
        (self.args.len() - at) as u32
    }

    /// A branch from the current block to `b`. A block not entered yet
    /// keeps the variables' values every branch into it agrees on.
    fn edge(&mut self, b: Block) {
        let bi = b.as_u32() as usize;
        let blk = &mut self.blocks[bi];
        blk.preds += 1;
        if blk.entered {
            // a back edge: what the block assumed on entry may not hold
            if self
                .pending_inherit
                .is_some_and(|(pb, _)| pb as usize == bi)
            {
                self.pending_inherit = None;
            }
            return;
        }
        let (at, n) = blk.inherit;
        if blk.preds == 1 {
            let at = self.snaps.len() as u32;
            self.snaps.extend_from_slice(&self.var_cur);
            self.blocks[bi].inherit = (at, self.var_cur.len() as u32);
        } else if at != NONE {
            for k in 0..n as usize {
                let s = &mut self.snaps[at as usize + k];
                if *s != self.var_cur[k] {
                    *s = NONE;
                }
            }
        }
    }

    fn restore(&mut self, (at, n): (u32, u32)) {
        self.var_cur[..n as usize].copy_from_slice(&self.snaps[at as usize..(at + n) as usize]);
    }

    fn ty_of_value(&self, x: Value) -> Ty {
        self.value_ty[v(x) as usize]
    }

    fn bin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        let ty = self.ty_of_value(x);
        self.def(Op::Bin(op), ty, v(x), v(y), NONE)
    }

    fn bin_imm(&mut self, op: BinOp, x: Value, y: impl Into<Imm64>) -> Value {
        let ty = self.ty_of_value(x);
        let imm = mask(ty, y.into().bits());
        self.def(Op::BinImm(op, imm), ty, v(x), NONE, NONE)
    }

    fn un(&mut self, op: UnOp, ty: Ty, x: Value) -> Value {
        self.def(Op::Un(op), ty, v(x), NONE, NONE)
    }

    fn callee(&mut self, addr: usize, sig: &Signature) -> Callee {
        let params_at = self.param_tys.len() as u32;
        self.param_tys
            .extend(sig.params.iter().map(|p| Ty::of(p.value_type)));
        Callee {
            addr,
            params_at,
            n_params: sig.params.len() as u32,
            ret: sig.returns.first().map(|r| Ty::of(r.value_type)),
        }
    }

    fn call_result(&mut self, inst: u32, ret: Option<Ty>) {
        if let Some(t) = ret {
            let d = self.new_value(t);
            let i = &mut self.insts[inst as usize];
            i.dst = d;
            i.res = val(d);
        }
    }
}

impl Ins for Lir {
    fn create_block(&mut self) -> Block {
        self.blocks.push(BlockData {
            first: NONE,
            last: NONE,
            inherit: (NONE, 0),
            ..BlockData::default()
        });
        Block::from_u32(self.blocks.len() as u32 - 1)
    }
    fn switch_to_block(&mut self, b: Block) {
        let bi = b.as_u32();
        self.cur = bi;
        let blk = &mut self.blocks[bi as usize];
        let inherit = if blk.entered { (NONE, 0) } else { blk.inherit };
        blk.entered = true;
        let sealed = blk.sealed;
        self.var_cur.iter_mut().for_each(|x| *x = NONE);
        // a block entered before it is sealed may still gain a predecessor
        // (a loop head's back edge); its values are known only once it is
        // sealed with nothing emitted yet
        if inherit.0 != NONE {
            if sealed {
                self.restore(inherit);
            } else {
                self.pending_inherit = Some((bi, inherit));
            }
        }
    }
    fn seal_block(&mut self, b: Block) {
        let bi = b.as_u32();
        let blk = &mut self.blocks[bi as usize];
        blk.sealed = true;
        if blk.first == NONE
            && bi == self.cur
            && let Some((pb, snap)) = self.pending_inherit.take()
            && pb == bi
        {
            self.restore(snap);
        }
    }
    fn append_block_param(&mut self, b: Block, ty: Type) -> Value {
        let d = self.new_value(Ty::of(ty));
        let blk = &mut self.blocks[b.as_u32() as usize];
        blk.params.push(d);
        blk.param_vals.push(val(d));
        val(d)
    }
    fn append_block_params_for_function_params(&mut self, b: Block) {
        let d = self.new_value(Ty::I64);
        self.arg0 = d;
        let blk = &mut self.blocks[b.as_u32() as usize];
        blk.params.push(d);
        blk.param_vals.push(val(d));
    }
    fn block_params(&self, b: Block) -> &[Value] {
        &self.blocks[b.as_u32() as usize].param_vals
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
    fn use_var(&mut self, var: Variable) -> Value {
        let n = var.as_u32();
        let known = self.var_cur[n as usize];
        if known != NONE {
            return val(known);
        }
        let x = self.def(Op::VarRead, self.var_ty[n as usize], n, NONE, NONE);
        self.var_cur[n as usize] = v(x);
        x
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
        self.def(Op::Iconst(mask(t, n.into().bits())), t, NONE, NONE, NONE)
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
        let t = self.ty_of_value(x);
        let d = self.new_value(Ty::I8);
        self.push(Op::Icmp(cc.into()), t, d, v(x), v(y), NONE);
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
        _flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value {
        self.def(Op::Load(off.into().into()), Ty::of(ty), v(p), NONE, NONE)
    }
    fn uload8(
        &mut self,
        ty: Type,
        _flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value {
        self.def(Op::Uload8(off.into().into()), Ty::of(ty), v(p), NONE, NONE)
    }
    fn store(
        &mut self,
        _flags: impl Into<MemFlagsData>,
        x: Value,
        p: Value,
        off: impl Into<Offset32>,
    ) -> CInst {
        let t = self.ty_of_value(x);
        CInst::from_u32(self.push(Op::Store(off.into().into()), t, NONE, v(x), v(p), NONE))
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
        self.edge(b);
        let i = self.push(Op::Jump, Ty::I64, NONE, b.as_u32(), NONE, NONE);
        let at = self.args.len() as u32;
        let n = self.push_block_args(args);
        let inst = &mut self.insts[i as usize];
        (inst.args_at, inst.n_args) = (at, n);
        CInst::from_u32(i)
    }
    fn brif<'a>(
        &mut self,
        c: Value,
        then_b: Block,
        then_args: impl IntoIterator<Item = &'a BlockArg>,
        else_b: Block,
        else_args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> CInst {
        self.edge(then_b);
        self.edge(else_b);
        let at = self.args.len() as u32;
        let n_then = self.push_block_args(then_args);
        let n_else = self.push_block_args(else_args);
        let i = self.push(
            Op::Brif(n_then),
            Ty::I8,
            NONE,
            v(c),
            then_b.as_u32(),
            else_b.as_u32(),
        );
        let inst = &mut self.insts[i as usize];
        (inst.args_at, inst.n_args) = (at, n_then + n_else);
        CInst::from_u32(i)
    }
    fn call(&mut self, f: FuncRef, args: &[Value]) -> CInst {
        let n = f.as_u32();
        let ret = self.funcs[n as usize].ret;
        let i = self.push(Op::Call, Ty::I64, NONE, n, NONE, NONE);
        self.with_args(i, args.iter().map(|x| v(*x)));
        self.call_result(i, ret);
        CInst::from_u32(i)
    }
    fn call_indirect(&mut self, sig: SigRef, callee: Value, args: &[Value]) -> CInst {
        let ret = self.sigs[sig.as_u32() as usize].ret;
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

impl Emit for Lir {
    fn make_signature(&self) -> Signature {
        Signature::new(CallConv::SystemV)
    }
    fn declare_function(
        &mut self,
        name: &str,
        _linkage: Linkage,
        sig: &Signature,
    ) -> ModuleResult<FuncId> {
        let addr = match crate::jit_backend::trace::trace_helper(name).or_else(|| libm(name)) {
            Some(a) => a as usize,
            None => {
                self.unsupported = Some("call to an unknown function");
                0
            }
        };
        let c = self.callee(addr, sig);
        self.funcs.push(c);
        Ok(FuncId::from_u32(self.funcs.len() as u32 - 1))
    }
    fn import_func(&mut self, id: FuncId) -> FuncRef {
        FuncRef::from_u32(id.as_u32())
    }
    fn import_signature(&mut self, sig: Signature) -> SigRef {
        let c = self.callee(0, &sig);
        self.sigs.push(c);
        SigRef::from_u32(self.sigs.len() as u32 - 1)
    }
    fn target_triple(&self) -> target_lexicon::Triple {
        target_lexicon::Triple::host()
    }
    fn declare_data(
        &mut self,
        name: &str,
        _linkage: Linkage,
        _writable: bool,
        _tls: bool,
    ) -> ModuleResult<DataId> {
        Err(ModuleError::Undeclared(name.to_owned()))
    }
    fn define_data(&mut self, _id: DataId, _desc: &DataDescription) -> ModuleResult<()> {
        unreachable!("only AOT lowering defines data, and it builds Cranelift IR")
    }
    fn declare_data_in_data(&mut self, _data: DataId, _ctx: &mut DataDescription) -> GlobalValue {
        unreachable!("only AOT lowering declares data, and it builds Cranelift IR")
    }
    fn symbol_value(&mut self, _ty: Type, _gv: GlobalValue) -> Value {
        unreachable!("only AOT lowering reads data symbols, and it builds Cranelift IR")
    }
    fn declare_data_in_func(&mut self, _data: DataId) -> GlobalValue {
        unreachable!("only AOT lowering declares data, and it builds Cranelift IR")
    }
}

/// The C math functions a folded `math.*` call reaches, which Cranelift's
/// JIT finds by symbol lookup in the process.
fn libm(name: &str) -> Option<*const u8> {
    unsafe extern "C" {
        fn sin(x: f64) -> f64;
        fn cos(x: f64) -> f64;
        fn tan(x: f64) -> f64;
        fn asin(x: f64) -> f64;
        fn acos(x: f64) -> f64;
        fn atan(x: f64) -> f64;
        fn atan2(y: f64, x: f64) -> f64;
        fn exp(x: f64) -> f64;
        fn log(x: f64) -> f64;
        fn sqrt(x: f64) -> f64;
        fn floor(x: f64) -> f64;
        fn ceil(x: f64) -> f64;
        fn pow(x: f64, y: f64) -> f64;
        fn fmod(x: f64, y: f64) -> f64;
    }
    Some(match name {
        "sin" => sin as *const u8,
        "cos" => cos as *const u8,
        "tan" => tan as *const u8,
        "asin" => asin as *const u8,
        "acos" => acos as *const u8,
        "atan" => atan as *const u8,
        "atan2" => atan2 as *const u8,
        "exp" => exp as *const u8,
        "log" => log as *const u8,
        "sqrt" => sqrt as *const u8,
        "floor" => floor as *const u8,
        "ceil" => ceil as *const u8,
        "pow" => pow as *const u8,
        "fmod" => fmod as *const u8,
        _ => return None,
    })
}
