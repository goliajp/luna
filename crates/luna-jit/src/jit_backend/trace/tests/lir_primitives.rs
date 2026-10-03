//! The baseline code generator against Cranelift, one primitive at a time.
//!
//! Each case is emitted through [`Ins`] twice: into a [`Lir`], which runs
//! as the baseline tier's machine code and again replayed as Cranelift IR
//! (what tier-up compiles), and straight into a Cranelift
//! `FunctionBuilder`. All three run on the same inputs and must leave the
//! same bits in the buffer they are handed: inputs in words 0..4, results
//! from word 8 on.

use super::super::lir::{self, CodeArena, Lir};
use super::*;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{MemFlagsData, SigRef, Signature};

#[path = "lir_primitives_cases.rs"]
mod cases;
use cases::{Case, emit};

/// Out of the results' way, so a store of the wrong width shows.
const FILL: i64 = 0x5a5a_5a5a_5a5a_5a5a;

/// Signatures for `call_indirect`, which [`Ins`] leaves to the module side.
trait Sigs: Ins {
    fn make_sig(&self) -> Signature;
    fn sig(&mut self, s: Signature) -> SigRef;
}

impl Sigs for Lir {
    fn make_sig(&self) -> Signature {
        Emit::make_signature(self)
    }
    fn sig(&mut self, s: Signature) -> SigRef {
        Emit::import_signature(self, s)
    }
}

impl Sigs for FunctionBuilder<'_> {
    fn make_sig(&self) -> Signature {
        Signature::new(cranelift_codegen::isa::CallConv::triple_default(
            &target_lexicon::Triple::host(),
        ))
    }
    fn sig(&mut self, s: Signature) -> SigRef {
        self.import_signature(s)
    }
}

fn mem() -> MemFlagsData {
    MemFlagsData::trusted()
}

/// The entry block with the buffer pointer; `case` then emits its body.
fn build<E: Sigs>(e: &mut E, case: &Case) {
    let entry = e.create_block();
    e.append_block_params_for_function_params(entry);
    e.switch_to_block(entry);
    e.seal_block(entry);
    let p = e.block_params(entry)[0];
    emit(e, case, p);
}

fn trace_sig(module: &JITModule) -> Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

/// The case through Cranelift directly.
fn cranelift(case: &Case) -> (JITModule, *const u8) {
    let mut module = build_trace_jit_module().expect("trace module");
    let sig = trace_sig(&module);
    let id = module
        .declare_function("case", Linkage::Local, &sig)
        .expect("declare");
    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    let mut fbc = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    build(&mut b, case);
    b.seal_all_blocks();
    b.finalize(module.target_config());
    module.define_function(id, &mut ctx).expect("define");
    module.finalize_definitions().expect("finalize");
    let f = module.get_finalized_function(id);
    (module, f)
}

/// The case through the baseline tier, and replayed as Cranelift IR.
fn baseline(case: &Case, arena: &mut CodeArena) -> (*const u8, JITModule, *const u8) {
    let mut l = Lir::new();
    build(&mut l, case);
    let code = lir::assemble(&l, arena)
        .unwrap_or_else(|why| panic!("{case:?}: the baseline tier refused it: {why}"));
    let mut module = build_trace_jit_module().expect("trace module");
    let id = lir::define_clif(&l, &mut module).expect("replay");
    module.finalize_definitions().expect("finalize");
    let f = module.get_finalized_function(id);
    (code, module, f)
}

fn run(code: *const u8, inputs: [i64; 4]) -> ([i64; 16], i64) {
    let mut buf = [FILL; 16];
    buf[..4].copy_from_slice(&inputs);
    // SAFETY: `code` was generated for the `TraceFn` ABI and stays mapped
    // while the case's module and arena live; the function reads words
    // 0..4 and writes words 8..16 of `buf`, which is 16 words long
    let r = unsafe { std::mem::transmute::<*const u8, TraceFn>(code)(buf.as_mut_ptr()) };
    (buf, r)
}

fn check(case: &Case, inputs: &[[i64; 4]]) {
    let mut arena = CodeArena::default();
    let (direct_module, direct) = cranelift(case);
    let (code, replay_module, replay) = baseline(case, &mut arena);
    for &input in inputs {
        let want = run(direct, input);
        assert_eq!(run(code, input), want, "{case:?} baseline on {input:x?}");
        assert_eq!(run(replay, input), want, "{case:?} replayed on {input:x?}");
    }
    drop((direct_module, replay_module));
    // SAFETY: the case's code is not running and is not entered again
    unsafe { arena.free() };
}

const INTS: [i64; 18] = [
    0,
    1,
    -1,
    2,
    7,
    31,
    32,
    63,
    64,
    65,
    255,
    256,
    -1000,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    i64::MAX,
    i64::MIN,
];

fn f(x: f64) -> i64 {
    x.to_bits() as i64
}

fn floats() -> Vec<i64> {
    [
        0.0,
        -0.0,
        1.5,
        -2.5,
        0.49999999999999994,
        -7.5,
        1e300,
        -1e-310,
        9.223372036854775807e18,
        -9.223372036854775808e18,
        4503599627370497.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ]
    .into_iter()
    .map(f)
    .collect()
}

fn pairs(xs: &[i64]) -> Vec<[i64; 4]> {
    xs.iter()
        .flat_map(|&x| xs.iter().map(move |&y| [x, y, 0, 0]))
        .collect()
}

fn int_pairs() -> Vec<[i64; 4]> {
    pairs(&INTS)
}

fn float_pairs() -> Vec<[i64; 4]> {
    pairs(&floats())
}

const INT_CCS: [IntCC; 10] = [
    IntCC::Equal,
    IntCC::NotEqual,
    IntCC::SignedLessThan,
    IntCC::SignedLessThanOrEqual,
    IntCC::SignedGreaterThan,
    IntCC::SignedGreaterThanOrEqual,
    IntCC::UnsignedLessThan,
    IntCC::UnsignedLessThanOrEqual,
    IntCC::UnsignedGreaterThan,
    IntCC::UnsignedGreaterThanOrEqual,
];

const FLOAT_CCS: [FloatCC; 6] = [
    FloatCC::Equal,
    FloatCC::NotEqual,
    FloatCC::LessThan,
    FloatCC::LessThanOrEqual,
    FloatCC::GreaterThan,
    FloatCC::GreaterThanOrEqual,
];

const IMMS: [i64; 9] = [
    0,
    1,
    -1,
    4095,
    4096,
    -4096,
    0xff00,
    0x1234_5678_9a,
    i64::MIN,
];

#[test]
fn integer_arithmetic_wraps_like_cranelift() {
    use cases::IntOp::*;
    for ty in [types::I64, types::I32, types::I16, types::I8] {
        for op in [Add, Sub, Mul, And, Or, Xor] {
            check(&Case::Int(op, ty), &int_pairs());
        }
        check(&Case::Neg(ty), &int_pairs());
        check(&Case::Not(ty), &int_pairs());
    }
    for ty in [types::I64, types::I32] {
        for op in [Shl, Ushr, Smin, Smax] {
            check(&Case::Int(op, ty), &int_pairs());
        }
    }
    let divisible: Vec<_> = int_pairs()
        .into_iter()
        .filter(|&[x, y, ..]| y != 0 && !(x == i64::MIN && y == -1))
        .collect();
    check(&Case::Int(Sdiv, types::I64), &divisible);
    check(&Case::Int(Udiv, types::I64), &divisible);
}

#[test]
fn immediates_of_every_size_match_cranelift() {
    use cases::ImmOp::*;
    let ins: Vec<_> = INTS.iter().map(|&x| [x, 0, 0, 0]).collect();
    for imm in IMMS {
        for op in [AddU, AddS, AndU, AndS, XorU] {
            check(&Case::Imm(op, imm), &ins);
        }
        for cc in INT_CCS {
            check(&Case::IcmpImm(cc, imm), &ins);
        }
    }
    for sh in [0, 1, 3, 31, 32, 63, 64, 65] {
        for op in [ShlU, UshrU, SshrU] {
            check(&Case::Imm(op, sh), &ins);
        }
    }
}

#[test]
fn integer_comparisons_set_and_branch_like_cranelift() {
    for ty in [types::I64, types::I32, types::I8] {
        for cc in INT_CCS {
            check(&Case::Icmp(cc, ty), &int_pairs());
            check(&Case::IcmpBranch(cc, ty), &int_pairs());
        }
    }
    check(&Case::Select, &int_pairs());
}

#[test]
fn float_operations_match_cranelift_on_nan_infinity_and_signed_zero() {
    use cases::FloatOp::*;
    for op in [Add, Sub, Mul, Div] {
        check(&Case::Float(op), &float_pairs());
    }
    for cc in FLOAT_CCS {
        check(&Case::Fcmp(cc), &float_pairs());
        check(&Case::FcmpBranch(cc), &float_pairs());
    }
    let ones: Vec<_> = floats().into_iter().map(|x| [x, 0, 0, 0]).collect();
    for op in [Neg, Floor, Ceil, ToSintSat, Bits] {
        check(&Case::FloatUn(op), &ones);
    }
    let in_range: Vec<_> = [0.0, -0.0, 1.5, -2.5, -7.99, 4503599627370497.0, -9.2e18]
        .into_iter()
        .map(|x| [f(x), 0, 0, 0])
        .collect();
    check(&Case::FloatUn(ToSint), &in_range);
}

#[test]
fn conversions_and_narrow_values_match_cranelift() {
    let ins: Vec<_> = INTS.iter().map(|&x| [x, 0, 0, 0]).collect();
    for ty in [types::I8, types::I16, types::I32] {
        check(&Case::Reduce(ty), &ins);
        check(&Case::Extend(ty), &ins);
        check(&Case::FromSint(ty), &ins);
    }
    check(&Case::FromSint(types::I64), &ins);
    check(&Case::IntToFloatBits, &ins);
}

#[test]
fn memory_and_stack_slots_match_cranelift() {
    let ins: Vec<_> = INTS.iter().map(|&x| [x, !x, 0, 0]).collect();
    for ty in [types::I8, types::I16, types::I32, types::I64, types::F64] {
        check(&Case::Memory(ty), &ins);
        check(&Case::Stack(ty), &ins);
    }
    check(&Case::Uload8, &ins);
}

#[test]
fn control_flow_variables_and_spills_match_cranelift() {
    let ins: Vec<_> = INTS
        .iter()
        .map(|&x| [x, x.wrapping_mul(7), (x & 63) + 1, 0])
        .collect();
    check(&Case::Swap, &ins);
    check(&Case::Loop, &ins);
    check(&Case::Pressure, &ins);
    check(&Case::Calls, &ins);
}
