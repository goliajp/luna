//! What the recorder notes about an op besides the op itself: the hash
//! slots a table access found its key in, the step sign of the loop that
//! ends the trace, and whether 5.1 / 5.2 arithmetic is left to the
//! interpreter.

use super::*;

impl Vm {
    /// For `GetField` / `SetField` / `Self`, the hash slot of the table
    /// operand holding the constant string key, if the key is there; for
    /// `GetTable` / `SetTable`, the slot of a string key in a register.
    pub(super) fn field_slot_of(&self, cl: Gc<LuaClosure>, inst: Inst, base: u32) -> Option<u32> {
        use crate::vm::isa::Op;
        let reg = |r: u32| self.stack.get((base + r) as usize).copied();
        let (t, key) = match inst.op() {
            Op::GetField | Op::SelfOp => {
                (inst.b(), cl.proto.consts.get(inst.c() as usize).copied())
            }
            Op::SetField => (inst.a(), cl.proto.consts.get(inst.b() as usize).copied()),
            // a string key in a register
            Op::GetTable => (inst.b(), reg(inst.c())),
            Op::SetTable => (inst.a(), reg(inst.b())),
            _ => return None,
        };
        let Value::Table(t) = reg(t)? else {
            return None;
        };
        let key @ Value::Str(_) = key? else {
            return None;
        };
        t.find_node_idx(key).map(|i| i as u32)
    }

    /// For a `SelfOp` whose receiver table lacks the constant key: the hash
    /// slot of `__index` in the receiver's metatable, when that holds a
    /// table, and of the key in that table.
    pub(super) fn index_slots_of(
        &self,
        cl: Gc<LuaClosure>,
        inst: Inst,
        base: u32,
    ) -> Option<(u32, u32)> {
        use crate::vm::isa::Op;
        if inst.op() != Op::SelfOp || !inst.k() {
            return None;
        }
        let Value::Table(t) = *self.stack.get((base + inst.b()) as usize)? else {
            return None;
        };
        let key @ Value::Str(_) = *cl.proto.consts.get(inst.c() as usize)? else {
            return None;
        };
        let mt = t.metatable()?;
        let index = Value::Str(self.mm_names[Mm::Index as usize]);
        let m = mt.find_node_idx(index)?;
        let Some(Value::Table(link)) = mt.node_val_at(m) else {
            return None;
        };
        let k = link.find_node_idx(key)?;
        Some((m as u32, k as u32))
    }

    /// Notes the step sign of the first depth-0 `ForLoop`, the one that
    /// ends the trace, when its step is an integer.
    pub(super) fn note_for_step(&mut self, inst: Inst, base: u32, cur_depth: usize) {
        use crate::vm::isa::Op;
        if inst.op() != Op::ForLoop || cur_depth != 0 {
            return;
        }
        let step = self.stack[(base + inst.a() + 2) as usize];
        let rec = self.jit.active_trace.as_mut().expect("recording");
        if rec
            .ops
            .iter()
            .any(|r| r.inline_depth == 0 && r.inst.op() == Op::ForLoop)
        {
            return;
        }
        rec.for_step_up = match step {
            Value::Int(s) => Some(s > 0),
            _ => None,
        };
    }

    /// True when `inst` is arithmetic on integers only (every operand,
    /// register or constant, is an integer) that the trace cannot do as
    /// the doubles do. `+` and `-` it can: it keeps the exact result while
    /// that is a double's value and leaves the trace otherwise.
    pub(super) fn int_arith(&self, proto: &crate::runtime::Proto, inst: Inst, base: u32) -> bool {
        use crate::vm::isa::Op;
        let is_int = |r: u32| matches!(self.stack[(base + r) as usize], Value::Int(_));
        let k_int = |k: u32| matches!(proto.consts.get(k as usize), Some(Value::Int(_)));
        match inst.op() {
            Op::Mul | Op::Mod => is_int(inst.b()) && is_int(inst.c()),
            Op::Unm => is_int(inst.b()),
            Op::MulK | Op::ModK => is_int(inst.b()) && k_int(inst.c()),
            _ => false,
        }
    }
}
