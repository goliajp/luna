//! The builder interface the trace lowerer emits through.
//!
//! One lowering serves two code generators: [`ClifEmit`] builds Cranelift IR
//! (the optimizing tier, and luna-aot's object files), [`super::lir::Lir`]
//! records the same instructions for the baseline tier. Both use Cranelift's
//! entity types (`Value`, `Block`, `Variable`, ...) as plain indices.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee64, Imm64, Offset32};
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, GlobalValue, Inst, InstBuilder, MemFlagsData, SigRef, Signature,
    StackSlot, StackSlotData, Type, Value,
};
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, ModuleResult};

/// Instructions, blocks, variables and stack slots.
pub(crate) trait Ins {
    /// Instructions are built on the builder itself; kept so call sites
    /// read like Cranelift's `bcx.ins().iadd(..)`.
    fn ins(&mut self) -> &mut Self {
        self
    }
    fn create_block(&mut self) -> Block;
    fn switch_to_block(&mut self, b: Block);
    fn seal_block(&mut self, b: Block);
    fn append_block_param(&mut self, b: Block, ty: Type) -> Value;
    fn append_block_params_for_function_params(&mut self, b: Block);
    fn block_params(&self, b: Block) -> &[Value];
    fn declare_var(&mut self, ty: Type) -> Variable;
    fn def_var(&mut self, var: Variable, v: Value);
    fn use_var(&mut self, var: Variable) -> Value;
    fn create_sized_stack_slot(&mut self, data: StackSlotData) -> StackSlot;
    fn inst_results(&self, inst: Inst) -> &[Value];
    fn resolve_aliases(&self, v: Value) -> Value;

    fn iconst(&mut self, ty: Type, n: impl Into<Imm64>) -> Value;
    fn f64const(&mut self, n: impl Into<Ieee64>) -> Value;
    fn iadd(&mut self, x: Value, y: Value) -> Value;
    fn isub(&mut self, x: Value, y: Value) -> Value;
    fn imul(&mut self, x: Value, y: Value) -> Value;
    fn sdiv(&mut self, x: Value, y: Value) -> Value;
    fn udiv(&mut self, x: Value, y: Value) -> Value;
    fn ineg(&mut self, x: Value) -> Value;
    fn smin(&mut self, x: Value, y: Value) -> Value;
    fn smax(&mut self, x: Value, y: Value) -> Value;
    fn band(&mut self, x: Value, y: Value) -> Value;
    fn bor(&mut self, x: Value, y: Value) -> Value;
    fn bxor(&mut self, x: Value, y: Value) -> Value;
    fn bnot(&mut self, x: Value) -> Value;
    fn ishl(&mut self, x: Value, y: Value) -> Value;
    fn ushr(&mut self, x: Value, y: Value) -> Value;
    fn iadd_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn iadd_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn band_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn band_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn bxor_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn ishl_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn ushr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn sshr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value;
    fn icmp(&mut self, cc: impl Into<IntCC>, x: Value, y: Value) -> Value;
    fn icmp_imm_u(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value;
    fn icmp_imm_s(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value;
    fn select(&mut self, c: Value, x: Value, y: Value) -> Value;
    fn uextend(&mut self, ty: Type, x: Value) -> Value;
    fn ireduce(&mut self, ty: Type, x: Value) -> Value;
    fn bitcast(&mut self, ty: Type, flags: impl Into<MemFlagsData>, x: Value) -> Value;
    fn fadd(&mut self, x: Value, y: Value) -> Value;
    fn fsub(&mut self, x: Value, y: Value) -> Value;
    fn fmul(&mut self, x: Value, y: Value) -> Value;
    fn fdiv(&mut self, x: Value, y: Value) -> Value;
    fn fneg(&mut self, x: Value) -> Value;
    fn floor(&mut self, x: Value) -> Value;
    fn ceil(&mut self, x: Value) -> Value;
    fn fcmp(&mut self, cc: impl Into<FloatCC>, x: Value, y: Value) -> Value;
    fn fcvt_from_sint(&mut self, ty: Type, x: Value) -> Value;
    fn fcvt_to_sint(&mut self, ty: Type, x: Value) -> Value;
    fn fcvt_to_sint_sat(&mut self, ty: Type, x: Value) -> Value;
    fn load(
        &mut self,
        ty: Type,
        flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value;
    fn uload8(
        &mut self,
        ty: Type,
        flags: impl Into<MemFlagsData>,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Value;
    fn store(
        &mut self,
        flags: impl Into<MemFlagsData>,
        x: Value,
        p: Value,
        off: impl Into<Offset32>,
    ) -> Inst;
    fn stack_addr(&mut self, ty: Type, ss: StackSlot, off: impl Into<Offset32>) -> Value;
    fn stack_load(
        &mut self,
        ptr_ty: Type,
        ty: Type,
        ss: StackSlot,
        off: impl Into<Offset32>,
    ) -> Value;
    fn stack_store(
        &mut self,
        ptr_ty: Type,
        x: Value,
        ss: StackSlot,
        off: impl Into<Offset32>,
    ) -> Inst;
    fn jump<'a>(&mut self, b: Block, args: impl IntoIterator<Item = &'a BlockArg>) -> Inst;
    fn brif<'a>(
        &mut self,
        c: Value,
        then_b: Block,
        then_args: impl IntoIterator<Item = &'a BlockArg>,
        else_b: Block,
        else_args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> Inst;
    fn call(&mut self, f: FuncRef, args: &[Value]) -> Inst;
    fn call_indirect(&mut self, sig: SigRef, callee: Value, args: &[Value]) -> Inst;
    fn return_(&mut self, rvals: &[Value]) -> Inst;
}

/// What the lowerer needs from the module a trace is lowered into: the
/// helpers it calls and, for luna-aot, the data objects string keys and
/// cells live in.
// the module methods keep the signatures of `cranelift_module::Module`,
// errors included
#[allow(clippy::result_large_err)]
pub(crate) trait Emit: Ins {
    fn make_signature(&self) -> Signature;
    fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        sig: &Signature,
    ) -> ModuleResult<FuncId>;
    fn import_func(&mut self, id: FuncId) -> FuncRef;
    fn import_signature(&mut self, sig: Signature) -> SigRef;
    // the data-object methods below are reached only when lowering for
    // luna-aot, which always builds Cranelift IR
    fn target_triple(&self) -> target_lexicon::Triple;
    fn declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> ModuleResult<DataId>;
    fn define_data(&mut self, id: DataId, desc: &DataDescription) -> ModuleResult<()>;
    fn declare_data_in_data(
        &mut self,
        data: DataId,
        ctx: &mut DataDescription,
    ) -> cranelift_codegen::ir::GlobalValue;
    fn symbol_value(&mut self, ty: Type, gv: GlobalValue) -> Value;
    fn declare_data_in_func(&mut self, data: DataId) -> GlobalValue;
    /// Adds one to the `u32` at `cell` and goes to `hot` when it reaches
    /// `at`, else to `cont`. Only the baseline tier counts: Cranelift IR
    /// just goes to `cont`.
    fn tier_count(&mut self, cell: i64, at: u32, hot: Block, cont: Block);
    /// The address `live`, which means something only in the Vm the trace
    /// is compiled for. The code another Vm installs gets that Vm's address
    /// of the same `kind` written over it (see `super::image`).
    fn reloc(&mut self, kind: RelocKind, live: i64) -> Value;
    /// Flags for a table's length state (`alimit`, the 5.5 length hint):
    /// in Cranelift IR a memory region of their own, so storing to them
    /// does not make the code load the table's other fields again.
    fn len_state_flags(&mut self) -> cranelift_codegen::ir::MemFlagsData;
}

/// The alias region of a table's length state (see
/// [`Emit::len_state_flags`]) in `func`.
pub(crate) fn len_state_flags_in(
    func: &mut cranelift_codegen::ir::Function,
) -> cranelift_codegen::ir::MemFlagsData {
    let region = func
        .dfg
        .alias_regions
        .insert(cranelift_codegen::ir::AliasRegionData {
            user_id: 0x4c454e,
            description: "table length state".into(),
        });
    cranelift_codegen::ir::MemFlagsData::trusted().with_alias_region(Some(region))
}

/// What a Vm-specific address in a trace's code stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RelocKind {
    /// An interned string (a key or a constant of one of the trace's protos).
    Str,
    /// A function prototype an inlined call is checked against.
    Proto,
    /// The frame chain of inline side exit `n`.
    Chain(u32),
    /// The baseline tier's iteration count.
    TierCell,
}

/// The name of the data symbol Cranelift code reads relocation `n` from.
pub(crate) fn reloc_symbol(n: usize) -> String {
    format!("__luna_reloc_{n}")
}

macro_rules! forward_ins {
    ($($field:ident)?) => {
        fn create_block(&mut self) -> Block { self$(.$field)?.create_block() }
        fn switch_to_block(&mut self, b: Block) { self$(.$field)?.switch_to_block(b) }
        fn seal_block(&mut self, b: Block) { self$(.$field)?.seal_block(b) }
        fn append_block_param(&mut self, b: Block, ty: Type) -> Value { self$(.$field)?.append_block_param(b, ty) }
        fn append_block_params_for_function_params(&mut self, b: Block) { self$(.$field)?.append_block_params_for_function_params(b) }
        fn block_params(&self, b: Block) -> &[Value] { self$(.$field)?.block_params(b) }
        fn declare_var(&mut self, ty: Type) -> Variable { self$(.$field)?.declare_var(ty) }
        fn def_var(&mut self, var: Variable, v: Value) { self$(.$field)?.def_var(var, v) }
        fn use_var(&mut self, var: Variable) -> Value { self$(.$field)?.use_var(var) }
        fn create_sized_stack_slot(&mut self, data: StackSlotData) -> StackSlot { self$(.$field)?.create_sized_stack_slot(data) }
        fn inst_results(&self, inst: Inst) -> &[Value] { self$(.$field)?.inst_results(inst) }
        fn resolve_aliases(&self, v: Value) -> Value { self$(.$field)?.func.dfg.resolve_aliases(v) }
        fn iconst(&mut self, ty: Type, n: impl Into<Imm64>) -> Value { self$(.$field)?.ins().iconst(ty, n) }
        fn f64const(&mut self, n: impl Into<Ieee64>) -> Value { self$(.$field)?.ins().f64const(n) }
        fn iadd(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().iadd(x, y) }
        fn isub(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().isub(x, y) }
        fn imul(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().imul(x, y) }
        fn sdiv(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().sdiv(x, y) }
        fn udiv(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().udiv(x, y) }
        fn ineg(&mut self, x: Value) -> Value { self$(.$field)?.ins().ineg(x) }
        fn smin(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().smin(x, y) }
        fn smax(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().smax(x, y) }
        fn band(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().band(x, y) }
        fn bor(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().bor(x, y) }
        fn bxor(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().bxor(x, y) }
        fn bnot(&mut self, x: Value) -> Value { self$(.$field)?.ins().bnot(x) }
        fn ishl(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().ishl(x, y) }
        fn ushr(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().ushr(x, y) }
        fn iadd_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().iadd_imm_u(x, y) }
        fn iadd_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().iadd_imm_s(x, y) }
        fn band_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().band_imm_u(x, y) }
        fn band_imm_s(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().band_imm_s(x, y) }
        fn bxor_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().bxor_imm_u(x, y) }
        fn ishl_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().ishl_imm_u(x, y) }
        fn ushr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().ushr_imm_u(x, y) }
        fn sshr_imm_u(&mut self, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().sshr_imm_u(x, y) }
        fn icmp(&mut self, cc: impl Into<IntCC>, x: Value, y: Value) -> Value { self$(.$field)?.ins().icmp(cc, x, y) }
        fn icmp_imm_u(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().icmp_imm_u(cc, x, y) }
        fn icmp_imm_s(&mut self, cc: IntCC, x: Value, y: impl Into<Imm64>) -> Value { self$(.$field)?.ins().icmp_imm_s(cc, x, y) }
        fn select(&mut self, c: Value, x: Value, y: Value) -> Value { self$(.$field)?.ins().select(c, x, y) }
        fn uextend(&mut self, ty: Type, x: Value) -> Value { self$(.$field)?.ins().uextend(ty, x) }
        fn ireduce(&mut self, ty: Type, x: Value) -> Value { self$(.$field)?.ins().ireduce(ty, x) }
        fn bitcast(&mut self, ty: Type, flags: impl Into<MemFlagsData>, x: Value) -> Value { self$(.$field)?.ins().bitcast(ty, flags, x) }
        fn fadd(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().fadd(x, y) }
        fn fsub(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().fsub(x, y) }
        fn fmul(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().fmul(x, y) }
        fn fdiv(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().fdiv(x, y) }
        fn fneg(&mut self, x: Value) -> Value { self$(.$field)?.ins().fneg(x) }
        fn floor(&mut self, x: Value) -> Value { self$(.$field)?.ins().floor(x) }
        fn ceil(&mut self, x: Value) -> Value { self$(.$field)?.ins().ceil(x) }
        fn fcmp(&mut self, cc: impl Into<FloatCC>, x: Value, y: Value) -> Value { self$(.$field)?.ins().fcmp(cc, x, y) }
        fn fcvt_from_sint(&mut self, ty: Type, x: Value) -> Value { self$(.$field)?.ins().fcvt_from_sint(ty, x) }
        fn fcvt_to_sint(&mut self, ty: Type, x: Value) -> Value { self$(.$field)?.ins().fcvt_to_sint(ty, x) }
        fn fcvt_to_sint_sat(&mut self, ty: Type, x: Value) -> Value { self$(.$field)?.ins().fcvt_to_sint_sat(ty, x) }
        fn load(&mut self, ty: Type, flags: impl Into<MemFlagsData>, p: Value, off: impl Into<Offset32>) -> Value { self$(.$field)?.ins().load(ty, flags, p, off) }
        fn uload8(&mut self, ty: Type, flags: impl Into<MemFlagsData>, p: Value, off: impl Into<Offset32>) -> Value { self$(.$field)?.ins().uload8(ty, flags, p, off) }
        fn store(&mut self, flags: impl Into<MemFlagsData>, x: Value, p: Value, off: impl Into<Offset32>) -> Inst { self$(.$field)?.ins().store(flags, x, p, off) }
        fn stack_addr(&mut self, ty: Type, ss: StackSlot, off: impl Into<Offset32>) -> Value { self$(.$field)?.ins().stack_addr(ty, ss, off) }
        fn stack_load(&mut self, ptr_ty: Type, ty: Type, ss: StackSlot, off: impl Into<Offset32>) -> Value { self$(.$field)?.ins().stack_load(ptr_ty, ty, ss, off) }
        fn stack_store(&mut self, ptr_ty: Type, x: Value, ss: StackSlot, off: impl Into<Offset32>) -> Inst { self$(.$field)?.ins().stack_store(ptr_ty, x, ss, off) }
        fn jump<'a>(&mut self, b: Block, args: impl IntoIterator<Item = &'a BlockArg>) -> Inst { self$(.$field)?.ins().jump(b, args) }
        fn brif<'a>(&mut self, c: Value, then_b: Block, then_args: impl IntoIterator<Item = &'a BlockArg>, else_b: Block, else_args: impl IntoIterator<Item = &'a BlockArg>) -> Inst { self$(.$field)?.ins().brif(c, then_b, then_args, else_b, else_args) }
        fn call(&mut self, f: FuncRef, args: &[Value]) -> Inst { self$(.$field)?.ins().call(f, args) }
        fn call_indirect(&mut self, sig: SigRef, callee: Value, args: &[Value]) -> Inst { self$(.$field)?.ins().call_indirect(sig, callee, args) }
        fn return_(&mut self, rvals: &[Value]) -> Inst { self$(.$field)?.ins().return_(rvals) }
    };
}

// a plain builder, for the method JIT's code shared with the trace lowerer
#[rustfmt::skip]
impl Ins for FunctionBuilder<'_> {
    forward_ins!();
}

/// Cranelift IR for a function in module `M`.
pub(crate) struct ClifEmit<'f, 'm, M: Module> {
    pub(crate) b: FunctionBuilder<'f>,
    pub(crate) m: &'m mut M,
    /// The relocations, in symbol order (see [`Emit::reloc`]).
    pub(crate) relocs: Vec<(RelocKind, i64)>,
}

#[rustfmt::skip]
impl<M: Module> Ins for ClifEmit<'_, '_, M> {
    forward_ins!(b);
}

impl<M: Module> Emit for ClifEmit<'_, '_, M> {
    fn len_state_flags(&mut self) -> cranelift_codegen::ir::MemFlagsData {
        len_state_flags_in(self.b.func)
    }
    fn make_signature(&self) -> Signature {
        self.m.make_signature()
    }
    fn declare_function(
        &mut self,
        name: &str,
        linkage: Linkage,
        sig: &Signature,
    ) -> ModuleResult<FuncId> {
        self.m.declare_function(name, linkage, sig)
    }
    fn import_func(&mut self, id: FuncId) -> FuncRef {
        self.m.declare_func_in_func(id, self.b.func)
    }
    fn import_signature(&mut self, sig: Signature) -> SigRef {
        self.b.func.import_signature(sig)
    }
    fn target_triple(&self) -> target_lexicon::Triple {
        self.m.isa().triple().clone()
    }
    fn declare_data(
        &mut self,
        name: &str,
        linkage: Linkage,
        writable: bool,
        tls: bool,
    ) -> ModuleResult<DataId> {
        self.m.declare_data(name, linkage, writable, tls)
    }
    fn define_data(&mut self, id: DataId, desc: &DataDescription) -> ModuleResult<()> {
        self.m.define_data(id, desc)
    }
    fn declare_data_in_data(&mut self, data: DataId, ctx: &mut DataDescription) -> GlobalValue {
        self.m.declare_data_in_data(data, ctx)
    }
    fn symbol_value(&mut self, ty: Type, gv: GlobalValue) -> Value {
        self.b.ins().symbol_value(ty, gv)
    }
    fn declare_data_in_func(&mut self, data: DataId) -> GlobalValue {
        self.m.declare_data_in_func(data, self.b.func)
    }
    fn tier_count(&mut self, _cell: i64, _at: u32, _hot: Block, cont: Block) {
        self.b.ins().jump(cont, &[]);
    }
    fn reloc(&mut self, kind: RelocKind, live: i64) -> Value {
        let n = match self.relocs.iter().position(|&r| r == (kind, live)) {
            Some(n) => n,
            None => {
                self.relocs.push((kind, live));
                self.relocs.len() - 1
            }
        };
        let id = self
            .m
            .declare_data(&reloc_symbol(n), Linkage::Import, false, false)
            .expect("declaring a relocation symbol");
        let gv = self.m.declare_data_in_func(id, self.b.func);
        self.b
            .ins()
            .symbol_value(cranelift_codegen::ir::types::I64, gv)
    }
}
