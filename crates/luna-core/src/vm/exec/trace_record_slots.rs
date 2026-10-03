//! Where a recorded table access found its key: the hash slots the
//! lowerer reads and writes directly.

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
}
