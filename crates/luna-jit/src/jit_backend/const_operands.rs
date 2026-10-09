//! The method JIT reads register operands only: a function's code is
//! rewritten with every constant- or immediate-operand opcode split into a
//! load and the register-operand opcode it abbreviates.

use luna_core::runtime::Value;
use luna_core::runtime::function::Proto;
use luna_core::vm::isa::{Inst, Op};

/// The registers above a frame's own that [`split_const_operands`] loads
/// operands into: an integer, a float and another one for each of two
/// operands (so none is pinned to two kinds), and one for an upvalue table.
pub(super) const SCRATCH_REGS: usize = 7;

/// `proto.code` with the constant operands, and the upvalue table of a
/// `GetTabUpR` / `SetTabUpR` / `SetTabUpK`, loaded into the scratch
/// registers from `first_scratch` up, and the jumps retargeted. `None`
/// when the split code cannot be expressed.
pub(super) fn split_const_operands(proto: &Proto, first_scratch: usize) -> Option<Vec<Inst>> {
    let code = &proto.code;
    if !code.iter().any(|&i| needs_split(i)) {
        return Some(code.to_vec());
    }
    // the scratch registers must fit an 8-bit operand
    if first_scratch + SCRATCH_REGS > 256 {
        return None;
    }
    let scratch = Scratch(first_scratch as u32);
    let mut out: Vec<Inst> = Vec::with_capacity(code.len() + 8);
    // old pc -> new pc, with one entry past the end
    let mut map: Vec<usize> = Vec::with_capacity(code.len() + 1);
    // new pc -> old pc of the instructions that carry a relative target
    let mut relative: Vec<(usize, usize)> = Vec::new();
    for (pc, &inst) in code.iter().enumerate() {
        map.push(out.len());
        match split(proto, inst, scratch) {
            Some(pair) => {
                // an instruction the one before it may skip must stay single
                if pc > 0 && skips_next(code[pc - 1].op()) {
                    return None;
                }
                out.extend(pair);
            }
            None => {
                let op = inst.op();
                if op == Op::Jmp
                    || op.is_for_prep()
                    || op.is_for_loop()
                    || op.is_tfor_prep()
                    || op.is_tfor_loop()
                {
                    relative.push((out.len(), pc));
                }
                out.push(inst);
            }
        }
    }
    map.push(out.len());
    for (new_pc, old_pc) in relative {
        let inst = out[new_pc];
        let at = |old_target: i64| -> Option<i64> {
            let t = usize::try_from(old_target).ok()?;
            map.get(t).map(|&n| n as i64)
        };
        let (old, new) = (old_pc as i64, new_pc as i64);
        out[new_pc] = match inst.op() {
            Op::Jmp => {
                let target = at(old + 1 + inst.sj() as i64)?;
                Inst::isj(Op::Jmp, i32::try_from(target - (new + 1)).ok()?)
            }
            // to the loop's `ForLoop`
            op if op.is_for_prep() => {
                let target = at(old + inst.bx() as i64)?;
                Inst::iabx(op, inst.a(), u32::try_from(target - new).ok()?)
            }
            // back to the loop body
            op if op.is_for_loop() || op.is_tfor_loop() => {
                let target = at(old + 1 - inst.bx() as i64)?;
                Inst::iabx(inst.op(), inst.a(), u32::try_from(new + 1 - target).ok()?)
            }
            // forward to the loop's `TForCall`
            _ => {
                let target = at(old + 1 + inst.bx() as i64)?;
                Inst::iabx(inst.op(), inst.a(), u32::try_from(target - (new + 1)).ok()?)
            }
        };
    }
    Some(out)
}

fn needs_split(inst: Inst) -> bool {
    matches!(
        inst.op(),
        Op::EqK | Op::LtK | Op::LeK | Op::EqKK | Op::LtKK | Op::LeKK
    ) || inst.arith_kk_op().is_some()
        || inst.split_const_operand(0).is_some()
        || table_operands(inst)
}

/// A table op with a constant operand or an upvalue table.
fn table_operands(inst: Inst) -> bool {
    match inst.op() {
        Op::SetTable | Op::SetField | Op::SetI | Op::SetTabUp => inst.k(),
        Op::GetTableK | Op::SetTableK | Op::GetTabUpR | Op::SetTabUpR | Op::SetTabUpK => true,
        _ => false,
    }
}

/// The scratch registers from the one given.
#[derive(Clone, Copy)]
struct Scratch(u32);

impl Scratch {
    /// The scratch register for constant `k` as operand `nth` (0 or 1),
    /// and the instruction loading it.
    fn load(self, proto: &Proto, k: u32, nth: u32) -> Option<(u32, Inst)> {
        let (reg, load) = match proto.consts.get(k as usize)? {
            Value::Int(_) => (self.0 + 2 * nth, Inst::iabx(Op::LoadK, 0, k)),
            Value::Float(_) => (self.0 + 2 * nth + 1, Inst::iabx(Op::LoadK, 0, k)),
            Value::Str(_) => (self.0 + 4 + nth, Inst::iabx(Op::LoadK, 0, k)),
            Value::Bool(true) => (self.0 + 4 + nth, Inst::iabc(Op::LoadTrue, 0, 0, 0, false)),
            Value::Bool(false) => (self.0 + 4 + nth, Inst::iabc(Op::LoadFalse, 0, 0, 0, false)),
            Value::Nil => (self.0 + 4 + nth, Inst::iabc(Op::LoadNil, 0, 0, 0, false)),
            _ => return None,
        };
        Some((reg, Inst(load.0 & !(0xFF << 7) | (reg << 7))))
    }

    /// The scratch register an upvalue table is loaded into.
    fn table(self) -> u32 {
        self.0 + 6
    }
}

/// The instructions `inst` stands for, its constant operands (and upvalue
/// table) loaded into scratch registers first.
fn split(proto: &Proto, inst: Inst, s: Scratch) -> Option<Vec<Inst>> {
    if table_operands(inst) {
        return split_table(proto, inst, s);
    }
    let float_const = |k: u32| matches!(proto.consts.get(k as usize), Some(Value::Float(_)));
    let (int_reg, float_reg) = (s.0, s.0 + 1);
    let (a, b, c, k) = (inst.a(), inst.b(), inst.c(), inst.k());
    let cmp = |op: Op| match op {
        Op::EqK | Op::EqKK => Op::Eq,
        Op::LtK | Op::LtKK => Op::Lt,
        _ => Op::Le,
    };
    match inst.op() {
        // `C`: the constant is the left operand (no metamethod can tell
        // the sides of an equality apart)
        op @ (Op::EqK | Op::LtK | Op::LeK) => {
            let (reg, load) = s.load(proto, b, 1)?;
            let (l, r) = if c != 0 && op != Op::EqK {
                (reg, a)
            } else {
                (a, reg)
            };
            return Some(vec![load, Inst::iabc(cmp(op), l, r, 0, k)]);
        }
        op @ (Op::EqKK | Op::LtKK | Op::LeKK) => {
            let (l, load_l) = s.load(proto, a, 0)?;
            let (r, load_r) = s.load(proto, b, 1)?;
            return Some(vec![load_l, load_r, Inst::iabc(cmp(op), l, r, 0, k)]);
        }
        op if op.arith_kk_op().is_some() => {
            let (l, load_l) = s.load(proto, b, 0)?;
            let (r, load_r) = s.load(proto, c, 1)?;
            let reg_op = op.arith_kk_op().expect("checked");
            return Some(vec![load_l, load_r, Inst::iabc(reg_op, a, l, r, false)]);
        }
        // a constant of any type, on the side `k` says
        op if op.arith_const_op().is_some()
            && !matches!(op, Op::AddI | Op::SubI | Op::ShrI | Op::ShlI) =>
        {
            let (reg, load) = s.load(proto, c, 1)?;
            let (l, r) = if k { (reg, b) } else { (b, reg) };
            let reg_op = op.arith_const_op().expect("checked");
            return Some(vec![load, Inst::iabc(reg_op, a, l, r, false)]);
        }
        _ => {}
    }
    let [load, _] = inst.split_const_operand(int_reg)?;
    let float = match load.op() {
        Op::LoadF => true,
        Op::LoadK => float_const(load.bx()),
        _ => false,
    };
    Some(
        inst.split_const_operand(if float { float_reg } else { int_reg })?
            .to_vec(),
    )
}

/// [`split`] for a table op: the register form (`GetTable`, `SetTable`,
/// `SetField`, `SetI`, `SetTabUp`) after the loads.
fn split_table(proto: &Proto, inst: Inst, s: Scratch) -> Option<Vec<Inst>> {
    let (a, b, c, k) = (inst.a(), inst.b(), inst.c(), inst.k());
    let mut out = Vec::with_capacity(4);
    let konst = |k: u32, nth: u32, out: &mut Vec<Inst>| -> Option<u32> {
        let (reg, load) = s.load(proto, k, nth)?;
        out.push(load);
        Some(reg)
    };
    let upval = |u: u32, out: &mut Vec<Inst>| {
        out.push(Inst::iabc(Op::GetUpval, s.table(), u, 0, false));
        s.table()
    };
    let last = match inst.op() {
        Op::SetTable | Op::SetField | Op::SetI | Op::SetTabUp => {
            let v = konst(c, 1, &mut out)?;
            Inst::iabc(inst.op(), a, b, v, false)
        }
        Op::GetTableK => {
            let key = konst(c, 0, &mut out)?;
            Inst::iabc(Op::GetTable, a, b, key, false)
        }
        Op::SetTableK => {
            let key = konst(b, 0, &mut out)?;
            let v = if k { konst(c, 1, &mut out)? } else { c };
            Inst::iabc(Op::SetTable, a, key, v, false)
        }
        Op::GetTabUpR => {
            let t = upval(b, &mut out);
            let key = if k { konst(c, 0, &mut out)? } else { c };
            Inst::iabc(Op::GetTable, a, t, key, false)
        }
        Op::SetTabUpR => {
            let t = upval(a, &mut out);
            let v = if k { konst(c, 1, &mut out)? } else { c };
            Inst::iabc(Op::SetTable, t, b, v, false)
        }
        Op::SetTabUpK => {
            let t = upval(a, &mut out);
            let key = konst(b, 0, &mut out)?;
            let v = if k { konst(c, 1, &mut out)? } else { c };
            Inst::iabc(Op::SetTable, t, key, v, false)
        }
        _ => unreachable!("table_operands"),
    };
    out.push(last);
    Some(out)
}

/// Opcodes that may skip the instruction after them.
fn skips_next(op: Op) -> bool {
    op.is_test() || matches!(op, Op::LFalseSkip | Op::LTrueSkip)
}
