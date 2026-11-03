//! A trace's instructions replayed as LLVM IR: the optimizing tier of the
//! LLVM backend (`LUNA_JIT_BACKEND=llvm`). Like [`super::clif`], it
//! compiles exactly what the baseline tier ran, with the same cells, exits
//! and helpers, so everything the shared lowering supports (inlined calls,
//! side exits, rooting of collectable values held in registers across
//! helper calls) reaches LLVM unchanged.
//!
//! Variables live in stack slots of their own that LLVM promotes to SSA;
//! block parameters become phi nodes; helpers are called by address.

use super::*;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::{Builder, BuilderError};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, IntType};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FloatValue, FunctionValue, IntValue,
    PhiValue, PointerValue,
};

mod body;
mod preds;
mod ops;

pub(super) type R<T> = Result<T, &'static str>;

pub(super) fn b<T>(r: Result<T, BuilderError>) -> R<T> {
    r.map_err(|_| "llvm:builder")
}

/// The function `start` names (see `clif::define`) compiled from `lir`,
/// and the pair that keeps its code mapped. `relocs`: the addresses the relocations stand for in
/// the Vm the code is for.
pub(crate) fn compile(
    lir: &Lir,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    start: super::Start,
) -> R<(*const u8, luna_jit_llvm::EnginePair)> {
    if let Some(u) = lir.unsupported {
        return Err(u);
    }
    let e = match (&lir.loop_entry, start) {
        (_, super::Start::Whole) => None,
        (Some(e), _) => Some(e),
        (None, _) => return Err("llvm:no-loop-entry"),
    };
    let mut an = live::Analysis::default();
    live::analyze_from(lir, &mut an, start);
    let (root, e, (reads, taken)) = match (e, start) {
        (Some(e), super::Start::Loop) => {
            let r = super::clif::live_in_vars(lir, &an, e).ok_or("llvm:loop-entry-reads")?;
            (e.block, Some(e), r)
        }
        _ => (0, None, (Vec::new(), Vec::new())),
    };
    luna_jit_llvm::compile_function(|ctx, module| {
        let builder = ctx.create_builder();
        let mut g = Gen::new(ctx, module, &builder, lir, relocs)?;
        g.start = start;
        g.body(&an, root, e, (&reads, &taken))
    })
}

pub(super) struct Gen<'c, 'a> {
    pub(super) ctx: &'c Context,
    pub(super) module: &'a Module<'c>,
    pub(super) b: &'a Builder<'c>,
    pub(super) lir: &'a Lir,
    pub(super) relocs: &'a [(crate::jit_backend::trace::RelocKind, i64)],
    pub(super) f: FunctionValue<'c>,
    pub(super) vals: Vec<Option<BasicValueEnum<'c>>>,
    pub(super) blocks: Vec<Option<BasicBlock<'c>>>,
    /// Per block: the phi of each of its parameters.
    pub(super) phis: Vec<Vec<PhiValue<'c>>>,
    pub(super) vars: Vec<PointerValue<'c>>,
    pub(super) slots: Vec<PointerValue<'c>>,
    /// The function compiled (see `clif::define`).
    pub(super) start: super::Start,
    /// In the loop's function, the home of each value the loop head takes
    /// from the back edges (see `LoopEntry::inherit`).
    pub(super) ihomes: Vec<Option<PointerValue<'c>>>,
}

impl<'c, 'a> Gen<'c, 'a> {
    fn new(
        ctx: &'c Context,
        module: &'a Module<'c>,
        b: &'a Builder<'c>,
        lir: &'a Lir,
        relocs: &'a [(crate::jit_backend::trace::RelocKind, i64)],
    ) -> R<Gen<'c, 'a>> {
        let i64t = ctx.i64_type();
        let f = module.add_function(
            luna_jit_llvm::ENTRY,
            i64t.fn_type(&[i64t.into()], false),
            None,
        );
        // the function's float operations are the constrained ones (see
        // `float_bin`)
        let strictfp = inkwell::attributes::Attribute::get_named_enum_kind_id("strictfp");
        f.add_attribute(
            inkwell::attributes::AttributeLoc::Function,
            ctx.create_enum_attribute(strictfp, 0),
        );
        Ok(Gen {
            ctx,
            module,
            b,
            lir,
            relocs,
            f,
            vals: vec![None; lir.value_ty.len()],
            blocks: vec![None; lir.blocks.len()],
            phis: vec![Vec::new(); lir.blocks.len()],
            vars: Vec::with_capacity(lir.var_ty.len()),
            slots: Vec::with_capacity(lir.slots.len()),
            start: super::Start::Whole,
            ihomes: Vec::new(),
        })
    }

    pub(super) fn ty(&self, t: Ty) -> BasicTypeEnum<'c> {
        match t {
            Ty::F64 => self.ctx.f64_type().into(),
            _ => self.ity(t).into(),
        }
    }

    pub(super) fn ity(&self, t: Ty) -> IntType<'c> {
        match t {
            Ty::I8 => self.ctx.i8_type(),
            Ty::I16 => self.ctx.i16_type(),
            Ty::I32 => self.ctx.i32_type(),
            Ty::I64 | Ty::F64 => self.ctx.i64_type(),
        }
    }

    pub(super) fn v(&self, n: u32) -> BasicValueEnum<'c> {
        self.vals[n as usize].expect("a value is defined before its uses in layout order")
    }

    pub(super) fn iv(&self, n: u32) -> IntValue<'c> {
        self.v(n).into_int_value()
    }

    pub(super) fn fv(&self, n: u32) -> FloatValue<'c> {
        self.v(n).into_float_value()
    }

    pub(super) fn set(&mut self, dst: u32, v: impl BasicValue<'c>) {
        self.vals[dst as usize] = Some(v.as_basic_value_enum());
    }

    /// The arguments of `i`.
    pub(super) fn args(&self, i: &Inst) -> Vec<BasicValueEnum<'c>> {
        self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize]
            .iter()
            .map(|&a| self.v(a))
            .collect()
    }

    /// A branch from the current block to `blk` passing `args`.
    pub(super) fn edge(&mut self, blk: u32, args: &[BasicValueEnum<'c>]) -> R<BasicBlock<'c>> {
        let from = self.b.get_insert_block().ok_or("llvm:builder")?;
        for (phi, a) in self.phis[blk as usize].iter().zip(args) {
            phi.add_incoming(&[(a as &dyn BasicValue, from)]);
        }
        self.blocks[blk as usize].ok_or("llvm:unreachable-target")
    }

    /// `i`'s callee, called with `args`.
    pub(super) fn call(&mut self, i: &Inst) -> R<()> {
        let lir = self.lir;
        let (callee, target) = match i.op {
            Op::Call => {
                let f = &lir.funcs[i.a as usize];
                (f, self.ctx.i64_type().const_int(f.addr as u64, false))
            }
            _ => (&lir.sigs[i.b as usize], self.iv(i.a)),
        };
        let params: Vec<BasicMetadataTypeEnum> = lir
            .params(callee)
            .iter()
            .map(|&t| self.ty(t).into())
            .collect();
        let fty = match callee.ret {
            Some(t) => self.ty(t).fn_type(&params, false),
            None => self.ctx.void_type().fn_type(&params, false),
        };
        let ptr = b(self
            .b
            .build_int_to_ptr(target, self.ctx.ptr_type(Default::default()), ""))?;
        let args: Vec<BasicMetadataValueEnum> = self.args(i).into_iter().map(Into::into).collect();
        let call = b(self.b.build_indirect_call(fty, ptr, &args, ""))?;
        if i.dst != NONE {
            match call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(r) => self.set(i.dst, r),
                _ => return Err("llvm:call-without-result"),
            }
        }
        Ok(())
    }

    /// The address `base + off` as a pointer.
    pub(super) fn addr(&self, base: IntValue<'c>, off: i32) -> R<PointerValue<'c>> {
        let a = if off == 0 {
            base
        } else {
            let o = self.ctx.i64_type().const_int(off as i64 as u64, true);
            b(self.b.build_int_add(base, o, ""))?
        };
        b(self
            .b
            .build_int_to_ptr(a, self.ctx.ptr_type(Default::default()), ""))
    }

    /// The address `off` bytes into stack slot `slot`.
    pub(super) fn slot_addr(&self, slot: u32, off: i32) -> R<IntValue<'c>> {
        let base = b(self
            .b
            .build_ptr_to_int(self.slots[slot as usize], self.ctx.i64_type(), ""))?;
        if off == 0 {
            return Ok(base);
        }
        let o = self.ctx.i64_type().const_int(off as i64 as u64, true);
        b(self.b.build_int_add(base, o, ""))
    }

    pub(super) fn strictfp(&self) -> inkwell::attributes::Attribute {
        let k = inkwell::attributes::Attribute::get_named_enum_kind_id("strictfp");
        self.ctx.create_enum_attribute(k, 0)
    }

    /// The intrinsic `name` overloaded on `tys`.
    pub(super) fn intrinsic(&self, name: &str, tys: &[BasicTypeEnum<'c>]) -> R<FunctionValue<'c>> {
        inkwell::intrinsics::Intrinsic::find(name)
            .and_then(|x| x.get_declaration(self.module, tys))
            .ok_or("llvm:intrinsic")
    }
}
