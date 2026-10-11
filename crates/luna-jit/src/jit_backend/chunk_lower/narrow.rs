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
//! `init`) run the float copy. A loop that integer kinds already type as
//! `Int` needs none of this, and an integer-valued float loop is the case
//! `e >= 0`.

use super::*;

/// A loop with a counted copy: its `ForPrep` and `ForLoop`, the blocks of
/// the copy (one per block of the body and the `ForLoop`, by pc; the
/// function's own blocks elsewhere), and the counter's variables.
pub(super) struct Counted {
    pub(super) prep: usize,
    pub(super) loop_pc: usize,
    pub(super) blocks: Vec<Option<Block>>,
    x: Variable,
    step: Variable,
    bound: Variable,
    q: Variable,
}

/// The loops of the chunk that get a counted copy: pre-5.3 float loops with
/// no loop inside them, in a chunk without self calls (whose ring of copies
/// would multiply the body again). Declares their variables and creates
/// the blocks of the copies.
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
    for &(prep, loop_pc, _) in &scan.for_loops {
        let float = matches!(reg_kinds.get(code[prep].a() as usize), Some(RegKind::Float));
        let inner = code[prep + 1..loop_pc]
            .iter()
            .any(|i| matches!(i.op(), Op::ForPrep | Op::ForPrep55 | Op::TForPrep));
        if code[prep].op() != Op::ForPrep || !float || inner {
            continue;
        }
        let mut blocks = pc_to_block.to_vec();
        for slot in &mut blocks[prep + 1..=loop_pc] {
            if slot.is_some() {
                *slot = Some(bcx.create_block());
            }
        }
        out.push(Counted {
            prep,
            loop_pc,
            blocks,
            x: bcx.declare_var(types::I64),
            step: bcx.declare_var(types::I64),
            bound: bcx.declare_var(types::I64),
            q: bcx.declare_var(types::F64),
        });
    }
    out
}

/// `2^p` from its exponent, for `|p| <= 1022`.
fn pow2(bcx: &mut FunctionBuilder<'_>, p: Value) -> Value {
    let biased = bcx.ins().iadd_imm_s(p, 1023);
    let bits = bcx.ins().ishl_imm_u(biased, 52);
    bcx.ins().bitcast(types::F64, MemFlagsData::new(), bits)
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
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::F64));
    sig.params.push(AbiParam::new(types::F64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function("luna_jit_for_scale", Linkage::Import, &sig)
        .ok()?;
    let callee = module.declare_func_in_func(id, bcx.func);
    let step_i = bcx.ins().iconst(types::I64, step);
    let call = bcx.ins().call(callee, &[init, limit, step_i]);
    let e = bcx.inst_results(call)[0];
    let on = bcx.ins().icmp_imm_s(IntCC::NotEqual, e, i64::MIN);
    let setup = bcx.create_block();
    bcx.ins().brif(on, setup, &[], float, &[]);
    bcx.seal_block(setup);
    bcx.switch_to_block(setup);
    // the scaled values: integers below 2^52 in magnitude (see
    // `luna_jit_for_scale`), so the conversions are exact and in range
    let q = pow2(bcx, e);
    let neg_e = bcx.ins().ineg(e);
    let inv_q = pow2(bcx, neg_e);
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
    let q = bcx.use_var(c.q);
    let next = bcx.ins().fmul(xf, q);
    next_into(bcx, next);
    bcx.ins().jump(body, &[]);
}
