//! Pre-5.3 float numeric loops get a second, integer-counted copy, chosen
//! once in `ForPrep` when every value the loop can reach is exact.
//!
//! The float loop steps `x += step` and tests `x <= limit` (`>=` for a
//! negative step), so a float add and a float compare sit on every
//! iteration. When all the loop's values are exactly representable, each
//! of those adds is exact, and the loop is the same as one on integers
//! scaled by a power of two: with `q = 2^e`, `X = x / q` steps by
//! `S = step / q` and the test is `X <= floor(limit / q)` (`>=` and `ceil`
//! for a negative step). The counted copy counts `X` in an integer register
//! and rebuilds `x = X * q` for the registers. The value, the test and
//! every register the interpreter can see are the float loop's, bit for
//! bit. The body is emitted twice, once per copy, so neither copy tests
//! which one it is on each iteration.
//!
//! When the scaled loop is exact is decided once in `ForPrep`, by
//! `luna_jit_for_scale`, which states the invariant; loops it rejects
//! (values too large for the scale, a NaN or infinite bound, a subnormal
//! `init`) run the float copy. When `init` and `limit` are numerals, it is
//! decided while compiling, and only one of the two loops is compiled. A loop that integer kinds already type as
//! `Int` needs none of this, and an integer-valued float loop is the case
//! `e >= 0`.

use super::*;

/// A loop with a counted copy: its `ForPrep` and `ForLoop`, the blocks of
/// the copy (one per block of the body and the `ForLoop`, by pc; the
/// function's own blocks elsewhere), and the counter's variables.
pub(super) struct Counted {
    pub(super) prep: usize,
    pub(super) loop_pc: usize,
    /// the counted form when `init` and `limit` are constants of the chunk:
    /// the loop is then only compiled counted, and `blocks` is empty
    pub(super) fixed: Option<luna_jit_helpers::ForScale>,
    pub(super) blocks: Vec<Option<Block>>,
    x: Variable,
    step: Variable,
    bound: Variable,
    q: Variable,
}

/// The loops of the chunk that get a counted copy: pre-5.3 float loops with
/// no loop inside them, in a chunk without self calls (whose ring of copies
/// would multiply the body again). Declares their variables and creates
/// the blocks of the copies. A loop whose `init` and `limit` are numerals
/// loaded right before its `ForPrep` is decided here instead: counted only
/// when its values are exact (it is then `fixed`, with no copy), else left
/// out and compiled as the float loop alone.
pub(super) fn plan(
    bcx: &mut FunctionBuilder<'_>,
    c: ChunkIn<'_>,
    scan: &ChunkScan,
    reg_kinds: &[RegKind],
    pc_to_block: &[Option<Block>],
    any_self_call: bool,
) -> Vec<Counted> {
    let code = c.code;
    let mut out = Vec::new();
    if !c.pre53 || any_self_call {
        return out;
    }
    for &(prep, loop_pc, step) in &scan.for_loops {
        let float = matches!(reg_kinds.get(code[prep].a() as usize), Some(RegKind::Float));
        let inner = code[prep + 1..loop_pc]
            .iter()
            .any(|i| matches!(i.op(), Op::ForPrep | Op::ForPrep55 | Op::TForPrep));
        if code[prep].op() != Op::ForPrep || !float || inner {
            continue;
        }
        let a = code[prep].a();
        let consts = (prep >= 3 && !scan.bb_starts[prep - 2..=prep].contains(&true))
            .then(|| Some((numeral(c, prep - 3, a)?, numeral(c, prep - 2, a + 1)?)))
            .flatten();
        let fixed = match consts {
            Some((init, limit)) => match luna_jit_helpers::for_scale_parts(init, limit, step) {
                Some(f) => Some(f),
                None => continue,
            },
            None => None,
        };
        let mut blocks = Vec::new();
        if fixed.is_none() {
            blocks = pc_to_block.to_vec();
            for slot in &mut blocks[prep + 1..=loop_pc] {
                if slot.is_some() {
                    *slot = Some(bcx.create_block());
                }
            }
        }
        out.push(Counted {
            prep,
            loop_pc,
            fixed,
            blocks,
            x: bcx.declare_var(types::I64),
            step: bcx.declare_var(types::I64),
            bound: bcx.declare_var(types::I64),
            q: bcx.declare_var(types::F64),
        });
    }
    out
}

/// The number instruction `pc` loads into register `reg`, when it is a
/// numeral load.
fn numeral(c: ChunkIn<'_>, pc: usize, reg: u32) -> Option<f64> {
    let i = c.code[pc];
    if i.a() != reg {
        return None;
    }
    match i.op() {
        Op::LoadI | Op::LoadF => Some(i.sbx() as f64),
        Op::LoadK => match c.proto.consts.get(i.bx() as usize)? {
            luna_core::runtime::Value::Float(f) => Some(*f),
            luna_core::runtime::Value::Int(n) => Some(*n as f64),
            _ => None,
        },
        _ => None,
    }
}

/// In `ForPrep` of a `fixed` loop: the counter, as constants. A scale
/// other than 1 is asked of `luna_jit_for_scale` at run time all the same:
/// as a constant, `q` would be rebuilt in the loop at every use (Cranelift
/// rematerializes constants), an extra float operation per iteration.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_fixed<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    c: &Counted,
    f: luna_jit_helpers::ForScale,
    init: Value,
    limit: Value,
    step: i64,
) -> Option<()> {
    let before = bcx.ins().iconst(types::I64, f.before);
    let s = bcx.ins().iconst(types::I64, f.step);
    let bound = bcx.ins().iconst(types::I64, f.bound);
    bcx.def_var(c.x, before);
    bcx.def_var(c.step, s);
    bcx.def_var(c.bound, bound);
    if f.e != 0 {
        // the bound too, else the loop rebuilds it from its constant at the
        // test of every iteration
        let q = scale_call(module, bcx, init, limit, step, 0)?;
        bcx.def_var(c.q, q);
        let bound_f = scale_call(module, bcx, init, limit, step, 1)?;
        let bound = bcx.ins().fcvt_to_sint_sat(types::I64, bound_f);
        bcx.def_var(c.bound, bound);
    }
    Some(())
}

/// A call of `luna_jit_for_scale(init, limit, step, part)`.
fn scale_call<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    init: Value,
    limit: Value,
    step: i64,
    part: i64,
) -> Option<Value> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::F64));
    sig.params.push(AbiParam::new(types::F64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::F64));
    let id = module
        .declare_function("luna_jit_for_scale", Linkage::Import, &sig)
        .ok()?;
    let callee = module.declare_func_in_func(id, bcx.func);
    let step_i = bcx.ins().iconst(types::I64, step);
    let part = bcx.ins().iconst(types::I64, part);
    let call = bcx.ins().call(callee, &[init, limit, step_i, part]);
    Some(bcx.inst_results(call)[0])
}

/// In `ForPrep`: go on to `counted` (the counted copy's `ForLoop`), with
/// the counter set up, when the loop `init, limit, step` can run it; else
/// to `float` (the float copy's).
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    c: &Counted,
    init: Value,
    limit: Value,
    step: i64,
    counted: Block,
    float: Block,
) -> Option<()> {
    let q = scale_call(module, bcx, init, limit, step, 0)?;
    let zero = float_const(bcx, 0.0);
    let on = bcx.ins().fcmp(FloatCC::NotEqual, q, zero);
    let setup = bcx.create_block();
    bcx.ins().brif(on, setup, &[], float, &[]);
    bcx.seal_block(setup);
    bcx.switch_to_block(setup);
    // the scaled values: integers below 2^52 in magnitude (see
    // `luna_jit_for_scale`), so the conversions are exact and in range
    let q_bits = bcx.ins().bitcast(types::I64, MemFlagsData::new(), q);
    // `1 / q`: the exponent field `1023 + e` becomes `1023 - e`
    let both = bcx.ins().iconst(types::I64, 2046 << 52);
    let inv_bits = bcx.ins().isub(both, q_bits);
    let inv_q = bcx.ins().bitcast(types::F64, MemFlagsData::new(), inv_bits);
    let init_s = bcx.ins().fmul(init, inv_q);
    let x0 = bcx.ins().fcvt_to_sint(types::I64, init_s);
    let step_f = float_const(bcx, step as f64);
    let step_s = bcx.ins().fmul(step_f, inv_q);
    let s = bcx.ins().fcvt_to_sint(types::I64, step_s);
    let limit_s = bcx.ins().fmul(limit, inv_q);
    let limit_s = if step > 0 {
        bcx.ins().floor(limit_s)
    } else {
        bcx.ins().ceil(limit_s)
    };
    let bound = bcx.ins().fcvt_to_sint(types::I64, limit_s);
    let before = bcx.ins().isub(x0, s);
    bcx.def_var(c.x, before);
    bcx.def_var(c.step, s);
    bcx.def_var(c.bound, bound);
    bcx.def_var(c.q, q);
    bcx.ins().jump(counted, &[]);
    Some(())
}

/// The counted copy's `ForLoop`: step `X`, leave for `exit` past the bound,
/// else hand `x` to `next_into` (which writes the registers) and go on to
/// the copy's `body`.
pub(super) fn step(
    bcx: &mut FunctionBuilder<'_>,
    c: &Counted,
    step: i64,
    body: Block,
    exit: Block,
    mut next_into: impl FnMut(&mut FunctionBuilder<'_>, Value),
) {
    let x = bcx.use_var(c.x);
    let s = bcx.use_var(c.step);
    let x = bcx.ins().iadd(x, s);
    bcx.def_var(c.x, x);
    let bound = bcx.use_var(c.bound);
    let cc = if step > 0 {
        IntCC::SignedLessThanOrEqual
    } else {
        IntCC::SignedGreaterThanOrEqual
    };
    let go = bcx.ins().icmp(cc, x, bound);
    let into = bcx.create_block();
    bcx.ins().brif(go, into, &[], exit, &[]);
    bcx.seal_block(into);
    bcx.switch_to_block(into);
    let xf = bcx.ins().fcvt_from_sint(types::F64, x);
    // an integer-valued loop known while compiling needs no scaling
    let next = if c.fixed.is_some_and(|f| f.e == 0) {
        xf
    } else {
        let q = bcx.use_var(c.q);
        bcx.ins().fmul(xf, q)
    };
    next_into(bcx, next);
    bcx.ins().jump(body, &[]);
}
