//! A recording's constant- and immediate-operand ops presented to the
//! lowerer as their register forms.

use luna_core::jit::trace_types::TraceRecord;
use luna_core::runtime::Value;
use luna_core::vm::isa::{Inst, Op};

/// The constant an op's virtual register holds.
#[derive(Clone, Copy, Debug)]
pub(super) enum VConst {
    Int(i64),
    Float(f64),
}

/// `record` with each constant- or immediate-operand op rewritten as the
/// register-operand op it abbreviates, reading its constant from register
/// `virt` (one past the frame), and per op the constant that register
/// holds. `None` when the record has no such op, or one whose constant
/// is not a number (left as it is, the lowerer refuses it).
pub(super) fn split_const_operands(
    record: &TraceRecord,
    virt: u32,
) -> Option<(TraceRecord, Vec<Option<VConst>>)> {
    let mut consts: Vec<Option<VConst>> = Vec::with_capacity(record.ops.len());
    let mut any = false;
    let mut out = record.clone();
    for rop in &mut out.ops {
        let inst = rop.inst;
        let Some((reg_form, k)) =
            register_form(inst, virt, |c| match rop.proto.consts.get(c as usize) {
                Some(&Value::Int(i)) => Some(VConst::Int(i)),
                Some(&Value::Float(f)) => Some(VConst::Float(f)),
                _ => None,
            })
        else {
            consts.push(None);
            continue;
        };
        rop.inst = reg_form;
        consts.push(Some(k?));
        any = true;
    }
    any.then_some((out, consts))
}

/// The register form of `inst` with its constant operand in `virt`, and
/// the constant (`None` inside: a `K` operand that is not a number).
fn register_form(
    inst: Inst,
    virt: u32,
    konst: impl Fn(u32) -> Option<VConst>,
) -> Option<(Inst, Option<VConst>)> {
    let (a, b) = (inst.a(), inst.b());
    if let Some(op) = inst.arith_const_op() {
        let k = match inst.op() {
            Op::AddI | Op::SubI | Op::ShrI | Op::ShlI => Some(VConst::Int(inst.sc() as i64)),
            _ => konst(inst.c()),
        };
        // `k`: the constant was the left operand
        let (l, r) = if inst.k() { (virt, b) } else { (b, virt) };
        return Some((Inst::iabc(op, a, l, r, false), k));
    }
    let (op, swap) = match inst.op() {
        Op::EqI => (Op::Eq, false),
        Op::LtI => (Op::Lt, false),
        Op::LeI => (Op::Le, false),
        Op::GtI => (Op::Lt, true),
        Op::GeI => (Op::Le, true),
        _ => return None,
    };
    let im = inst.sb();
    let k = if inst.c() != 0 {
        VConst::Float(im as f64)
    } else {
        VConst::Int(im as i64)
    };
    let (l, r) = if swap { (virt, a) } else { (a, virt) };
    Some((Inst::iabc(op, l, r, 0, inst.k()), Some(k)))
}
