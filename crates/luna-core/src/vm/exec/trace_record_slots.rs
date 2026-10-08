//! What the recorder notes about an op besides the op itself: the hash
//! slots a table access found its key in and the step sign of the loop
//! that ends the trace.

use super::*;

impl Vm {
    /// For `GetField` / `SetField` / `Self`, the hash slot of the table
    /// operand holding the constant string key, if the key is there; for
    /// `GetTable` / `SetTable`, the slot of a string key in a register, and
    /// for `GetTableK` / `SetTableK` of a constant string key; for
    /// `GetTabUp` / `SetTabUp`, the slot of the key in the upvalue table.
    pub(super) fn field_slot_of(&self, cl: Gc<LuaClosure>, inst: Inst, base: u32) -> Option<u32> {
        use crate::vm::isa::Op;
        let reg = |r: u32| self.stack.get((base + r) as usize).copied();
        let k = |i: u32| cl.proto.consts.get(i as usize).copied();
        let up = |u: u32| ((u as usize) < cl.upvals().len()).then(|| self.upval_get(cl, u));
        let (t, key) = match inst.op() {
            Op::GetField | Op::SelfOp => (reg(inst.b()), k(inst.c())),
            Op::SetField => (reg(inst.a()), k(inst.b())),
            // a string key in a register
            Op::GetTable => (reg(inst.b()), reg(inst.c())),
            Op::SetTable => (reg(inst.a()), reg(inst.b())),
            // a constant key of any type
            Op::GetTableK => (reg(inst.b()), k(inst.c())),
            Op::SetTableK => (reg(inst.a()), k(inst.b())),
            // a field of an upvalue table
            Op::GetTabUp => (up(inst.b()), k(inst.c())),
            Op::SetTabUp => (up(inst.a()), k(inst.b())),
            _ => return None,
        };
        let Value::Table(t) = t? else {
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
        if !inst.op().is_for_loop() || cur_depth != 0 {
            return;
        }
        // the step is the register before the loop variable
        let var = inst.op().for_layout().expect("a loop op").var();
        let step = self.stack[(base + inst.a() + var - 1) as usize];
        let rec = self.jit.active_trace.as_mut().expect("recording");
        if rec
            .ops
            .iter()
            .any(|r| r.inline_depth == 0 && r.inst.op().is_for_loop())
        {
            return;
        }
        rec.for_step_up = match step {
            Value::Int(s) => Some(s > 0),
            _ => None,
        };
    }
}
