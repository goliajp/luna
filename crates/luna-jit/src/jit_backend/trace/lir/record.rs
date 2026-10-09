//! The bookkeeping behind [`super::build`]: appending instructions, block
//! edges and variable snapshots, and the module side of [`Emit`].

use super::*;
use crate::jit_backend::trace::Emit;
use cranelift_codegen::entity::packed_option::ReservedValue;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, GlobalValue, SigRef, Signature, Type, Value,
};
use cranelift_codegen::isa::CallConv;
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, ModuleError, ModuleResult};

pub(super) fn v(x: Value) -> u32 {
    x.as_u32()
}

pub(super) fn val(n: u32) -> Value {
    Value::from_u32(n)
}

pub(super) fn mask(ty: Ty, n: i64) -> i64 {
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

    pub(super) fn new_value(&mut self, ty: Ty) -> u32 {
        self.value_ty.push(ty);
        self.konst.push(None);
        (self.value_ty.len() - 1) as u32
    }

    pub(super) fn push(&mut self, op: Op, ty: Ty, dst: u32, a: u32, b: u32, c: u32) -> u32 {
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

    pub(super) fn def(&mut self, op: Op, ty: Ty, a: u32, b: u32, c: u32) -> Value {
        let d = self.new_value(ty);
        self.push(op, ty, d, a, b, c);
        val(d)
    }

    pub(super) fn with_args(&mut self, inst: u32, args: impl IntoIterator<Item = u32>) {
        let at = self.args.len() as u32;
        self.args.extend(args);
        let i = &mut self.insts[inst as usize];
        i.args_at = at;
        i.n_args = self.args.len() as u32 - at;
    }

    pub(super) fn push_block_args<'a>(
        &mut self,
        args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> u32 {
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
    pub(super) fn edge(&mut self, b: Block) {
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

    /// Appends `d` to block `b`'s parameters, moving them to the end of the
    /// pool first when another block's were appended since.
    pub(super) fn add_block_param(&mut self, b: u32, d: u32) {
        let (at, n) = {
            let blk = &self.blocks[b as usize];
            (blk.params_at as usize, blk.n_params as usize)
        };
        if n == 0 || at + n != self.bparams.len() {
            let new_at = self.bparams.len();
            self.bparams.extend_from_within(at..at + n);
            self.bparam_vals.extend_from_within(at..at + n);
            self.blocks[b as usize].params_at = new_at as u32;
        }
        self.bparams.push(d);
        self.bparam_vals.push(val(d));
        self.blocks[b as usize].n_params += 1;
    }

    pub(super) fn restore(&mut self, (at, n): (u32, u32)) {
        self.var_cur[..n as usize].copy_from_slice(&self.snaps[at as usize..(at + n) as usize]);
    }

    fn reloc_index(&mut self, kind: super::super::RelocKind, live: i64) -> u32 {
        match self.relocs.iter().position(|&r| r == (kind, live)) {
            Some(n) => n as u32,
            None => {
                self.relocs.push((kind, live));
                self.relocs.len() as u32 - 1
            }
        }
    }

    pub(super) fn ty_of_value(&self, x: Value) -> Ty {
        self.value_ty[v(x) as usize]
    }

    /// `x op y`, in the immediate form when an operand is a constant.
    pub(super) fn bin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        let ty = self.ty_of_value(x);
        let imm_ok = matches!(
            op,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::And
                | BinOp::Or
                | BinOp::Xor
                | BinOp::Shl
                | BinOp::Ushr
                | BinOp::Sshr
        );
        let commutes = matches!(
            op,
            BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor
        );
        if imm_ok {
            if let Some(c) = self.konst[v(y) as usize] {
                return self.def(Op::BinImm(op, c), ty, v(x), NONE, v(y));
            }
            if commutes && let Some(c) = self.konst[v(x) as usize] {
                return self.def(Op::BinImm(op, c), ty, v(y), NONE, v(x));
            }
        }
        self.def(Op::Bin(op), ty, v(x), v(y), NONE)
    }

    pub(super) fn bin_imm(&mut self, op: BinOp, x: Value, y: impl Into<Imm64>) -> Value {
        let ty = self.ty_of_value(x);
        let imm = mask(ty, y.into().bits());
        self.def(Op::BinImm(op, imm), ty, v(x), NONE, NONE)
    }

    pub(super) fn un(&mut self, op: UnOp, ty: Ty, x: Value) -> Value {
        self.def(Op::Un(op), ty, v(x), NONE, NONE)
    }

    pub(super) fn callee(&mut self, addr: usize, sig: &Signature) -> Callee {
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

    pub(super) fn call_result(&mut self, inst: u32, ret: Option<Ty>) {
        if let Some(t) = ret {
            let d = self.new_value(t);
            let i = &mut self.insts[inst as usize];
            i.dst = d;
            i.res = val(d);
        }
    }
}

impl Emit for Lir {
    fn len_state_flags(&mut self) -> cranelift_codegen::ir::MemFlagsData {
        cranelift_codegen::ir::MemFlagsData::trusted()
    }
    fn make_signature(&self) -> Signature {
        Signature::new(CallConv::SystemV)
    }
    fn declare_function(
        &mut self,
        name: &str,
        _linkage: Linkage,
        sig: &Signature,
    ) -> ModuleResult<FuncId> {
        let addr = match crate::jit_backend::trace::trace_helper(name)
            .or_else(|| crate::jit_backend::math_fold::libm(name))
        {
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
    fn tier_count(&mut self, cell: i64, at: u32, hot: Block, cont: Block) {
        self.edge(hot);
        self.edge(cont);
        let n = self.reloc_index(super::super::RelocKind::TierCell, cell);
        let op = Op::TierCount { n, at };
        self.push(op, Ty::I64, NONE, NONE, hot.as_u32(), cont.as_u32());
    }
    fn reloc(&mut self, kind: super::super::RelocKind, live: i64) -> Value {
        let n = self.reloc_index(kind, live);
        self.def(Op::Reloc(n), Ty::I64, NONE, NONE, NONE)
    }
}
