//! The [`Ins`] implementations that forward to a Cranelift builder.

use super::{ClifEmit, Ins};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee64, Imm64, Offset32};
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, Inst, InstBuilder, MemFlagsData, SigRef, StackSlot, StackSlotData,
    Type, Value,
};
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_module::Module;

macro_rules! forward_ins {
    ($($field:ident)?) => {
        fn create_block(&mut self) -> Block { self$(.$field)?.create_block() }
        fn switch_to_block(&mut self, b: Block) { self$(.$field)?.switch_to_block(b) }
        fn seal_block(&mut self, b: Block) { self$(.$field)?.seal_block(b) }
        fn set_cold_block(&mut self, b: Block) { self$(.$field)?.set_cold_block(b) }
        fn current_block(&self) -> Option<Block> { self$(.$field)?.current_block() }
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
        fn umulhi(&mut self, x: Value, y: Value) -> Value { self$(.$field)?.ins().umulhi(x, y) }
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

#[rustfmt::skip]
impl<M: Module> Ins for ClifEmit<'_, '_, M> {
    forward_ins!(b);
}
