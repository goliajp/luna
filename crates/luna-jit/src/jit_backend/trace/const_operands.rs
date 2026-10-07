//! A recording's constant- and immediate-operand ops presented to the
//! lowerer as their register forms.

use luna_core::jit::trace_types::TraceRecord;
use luna_core::runtime::Gc;
use luna_core::runtime::Value;
use luna_core::runtime::function::Proto;
use luna_core::runtime::string::LuaStr;
use luna_core::vm::isa::{Inst, Op};

/// The constant an op's virtual register holds.
#[derive(Clone, Copy, Debug)]
pub(super) enum VConst {
    Int(i64),
    Float(f64),
    Str(Gc<LuaStr>),
    Bool(bool),
    Nil,
}

/// What an op's virtual register holds: a constant, or the table in an
/// upvalue of the op's function.
#[derive(Clone, Copy, Debug)]
pub(super) enum VSrc {
    Const(VConst),
    Upval(u32),
}

/// The virtual registers an op may read.
pub(super) const NVIRT: usize = 3;

/// Per op, what its virtual registers `virt`, `virt + 1` and `virt + 2`
/// hold.
pub(super) type VRegs = [Option<VSrc>; NVIRT];

/// `record` with each op that has a constant operand (or an upvalue table)
/// rewritten as the register-operand op it abbreviates, reading the
/// operand from a virtual register from `virt` (one past the frame) up,
/// and per op what those registers hold. `None` when the record has no
/// such op, or one this cannot express (left as it is, the lowerer
/// refuses it).
pub(super) fn split_const_operands(
    record: &TraceRecord,
    virt: u32,
) -> Option<(TraceRecord, Vec<VRegs>)> {
    let mut vregs: Vec<VRegs> = Vec::with_capacity(record.ops.len());
    let mut any = false;
    let mut out = record.clone();
    for rop in &mut out.ops {
        let inst = rop.inst;
        let konst = |c: u32| match rop.proto.consts.get(c as usize) {
            Some(&Value::Int(i)) => Some(VConst::Int(i)),
            Some(&Value::Float(f)) => Some(VConst::Float(f)),
            Some(&Value::Str(s)) => Some(VConst::Str(s)),
            Some(&Value::Bool(b)) => Some(VConst::Bool(b)),
            Some(&Value::Nil) => Some(VConst::Nil),
            _ => None,
        };
        if let Some(t) = table_form(inst, &rop.proto, virt, konst) {
            let (reg_form, v) = t?;
            rop.inst = reg_form;
            vregs.push(v);
            any = true;
            continue;
        }
        let number = |c: u32| konst(c).filter(|k| matches!(k, VConst::Int(_) | VConst::Float(_)));
        let Some((reg_form, k)) = register_form(inst, virt, number) else {
            vregs.push([None; NVIRT]);
            continue;
        };
        rop.inst = reg_form;
        vregs.push([Some(VSrc::Const(k?)), None, None]);
        any = true;
    }
    any.then_some((out, vregs))
}

/// The register form of a table op with a constant operand or an upvalue
/// table: `GetTable` / `GetField`, or a store from `R[C]` into a table in a
/// register. `None` for any other op; `Some(None)` when the virtual
/// registers do not fit an operand or a constant is not one a register
/// holds.
///
/// A field of an upvalue that is not `_ENV` (`GetTabUp` / `SetTabUp` since
/// 5.4, and since 5.2 in stores) is read and written as a field of a table
/// in a register, as the code before those forms did; a global stays
/// `GetTabUp`, which the math folds look for.
fn table_form(
    inst: Inst,
    proto: &Proto,
    virt: u32,
    konst: impl Fn(u32) -> Option<VConst>,
) -> Option<Option<(Inst, VRegs)>> {
    let (a, b, c, k) = (inst.a(), inst.b(), inst.c(), inst.k());
    let env = |u: u32| {
        proto
            .upvals
            .get(u as usize)
            .is_some_and(|d| d.name.as_str() == "_ENV")
    };
    match inst.op() {
        Op::GetTabUp if !env(b) => {}
        Op::SetTabUp if !env(a) => {}
        Op::SetTable | Op::SetField | Op::SetI | Op::SetTabUp if k => {}
        Op::GetTableK | Op::SetTableK | Op::GetTabUpR | Op::SetTabUpR | Op::SetTabUpK => {}
        _ => return None,
    }
    if virt as usize + NVIRT > 256 {
        return Some(None);
    }
    let mut v: VRegs = [None; NVIRT];
    let mut put = |j: usize, src: Option<VSrc>| -> Option<u32> {
        v[j] = Some(src?);
        Some(virt + j as u32)
    };
    let k_of = |x: u32| konst(x).map(VSrc::Const);
    let up = |x: u32| Some(VSrc::Upval(x));
    // the register a value operand `R[C]/K[C]` is read from
    let form = (|| {
        Some(match inst.op() {
            Op::GetTabUp if !env(b) => Inst::iabc(Op::GetField, a, put(0, up(b))?, c, k),
            Op::SetTabUp if !env(a) => {
                let t = put(0, up(a))?;
                let val = if k { put(2, k_of(c))? } else { c };
                Inst::iabc(Op::SetField, t, b, val, false)
            }
            Op::SetTable | Op::SetField | Op::SetI | Op::SetTabUp => {
                let val = put(2, k_of(c))?;
                Inst::iabc(inst.op(), a, b, val, false)
            }
            Op::GetTableK => Inst::iabc(Op::GetTable, a, b, put(1, k_of(c))?, false),
            Op::SetTableK => {
                let key = put(1, k_of(b))?;
                let val = if k { put(2, k_of(c))? } else { c };
                Inst::iabc(Op::SetTable, a, key, val, false)
            }
            Op::GetTabUpR => {
                let t = put(0, up(b))?;
                let key = if k { put(1, k_of(c))? } else { c };
                Inst::iabc(Op::GetTable, a, t, key, false)
            }
            Op::SetTabUpR => {
                let t = put(0, up(a))?;
                let val = if k { put(2, k_of(c))? } else { c };
                Inst::iabc(Op::SetTable, t, b, val, false)
            }
            Op::SetTabUpK => {
                let t = put(0, up(a))?;
                let key = put(1, k_of(b))?;
                let val = if k { put(2, k_of(c))? } else { c };
                Inst::iabc(Op::SetTable, t, key, val, false)
            }
            _ => unreachable!("matched above"),
        })
    })();
    Some(form.map(|f| (f, v)))
}

/// The kind of value a virtual register holds.
pub(super) fn vsrc_kind(src: VSrc) -> super::kinds::RegKind {
    use super::kinds::RegKind;
    match src {
        VSrc::Const(VConst::Int(_)) => RegKind::Int,
        VSrc::Const(VConst::Float(_)) => RegKind::Float,
        VSrc::Const(VConst::Str(_)) => RegKind::Str,
        VSrc::Const(VConst::Bool(_)) => RegKind::Bool,
        VSrc::Const(VConst::Nil) => RegKind::Nil,
        VSrc::Upval(_) => RegKind::Table,
    }
}

/// What register `r` of op `i` holds when it is one of the op's virtual
/// registers (from `virt` up).
pub(super) fn virt_at(vregs: &[VRegs], i: usize, r: u32, virt: usize) -> Option<VSrc> {
    let j = (r as usize).checked_sub(virt)?;
    vregs.get(i)?.get(j).copied().flatten()
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
