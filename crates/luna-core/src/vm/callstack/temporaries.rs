//! The C temporaries of a protected call's level: the values PUC's
//! pcall / xpcall and lua.c's `docall` keep below the function they call.

use crate::runtime::function::{CallFrame, ContKind, NativeCont};
use crate::runtime::{Builtin, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;

use super::{DbgKind, LocalSlot, ThreadStack};

impl Vm {
    /// The `n`-th local of level `i`, continuation `fi`: lua.c's `docall`
    /// keeps `pmain`'s arguments and the handler on the stack, the others
    /// values luna holds apart; past them, what a vararg function leaves.
    pub(super) fn cont_local(
        &self,
        ts: &ThreadStack,
        i: usize,
        fi: usize,
        n: i64,
    ) -> Option<(String, LocalSlot)> {
        let k = usize::try_from(n).ok()?.checked_sub(1)?;
        let name = self.temporary_locvar_name().to_string();
        let below = match i.checked_sub(1) {
            Some(j) if self.is_docall(ts, fi) => ts.func_slot(j)? as usize - 1,
            _ => 0,
        };
        if k < below {
            return Some((name, LocalSlot::Stack(k + 1)));
        }
        let held = if below > 0 {
            vec![]
        } else {
            self.cont_temporaries(ts, fi)
        };
        let k = k - below;
        let val = match held.get(k) {
            Some(&v) => v,
            None => *self.vararg_left(ts, i).get(k - held.len())?,
        };
        Some((name, LocalSlot::Held(val)))
    }

    /// Whether continuation `fi` is lua.c's `docall` from the bottom slot.
    fn is_docall(&self, ts: &ThreadStack, fi: usize) -> bool {
        let at = |nc: &NativeCont| ts.stack[nc.func_slot as usize];
        matches!(&ts.frames[fi], CallFrame::Cont(nc) if self.g.host_slots > 0 && nc.func_slot == 0
            && matches!(at(nc), Value::Native(n) if n.builtin == Builtin::HostXpcallInC))
    }

    /// The values PUC's pcall / xpcall / pairs keep below the function they
    /// call — their C temporaries (`luaB_pcall` inserts its status boolean,
    /// `luaB_xpcall` 5.3+ also its function and handler, 5.1/5.2 the handler).
    fn cont_temporaries(&self, ts: &ThreadStack, fi: usize) -> Vec<Value> {
        let v = self.version();
        let CallFrame::Cont(nc) = &ts.frames[fi] else {
            unreachable!("continuation level")
        };
        match nc.kind {
            ContKind::Pcall { .. } if v == LuaVersion::Lua51 => vec![],
            ContKind::Pcall { .. } if v == LuaVersion::Lua52 => vec![Value::Nil],
            ContKind::Pcall { .. } => vec![Value::Bool(true)],
            ContKind::Xpcall { handler, .. } if v <= LuaVersion::Lua52 => vec![handler],
            ContKind::Xpcall { handler, .. } => {
                vec![
                    ts.stack[(nc.func_slot + 1) as usize],
                    handler,
                    Value::Bool(true),
                ]
            }
            _ => vec![],
        }
    }

    /// What a 5.4+ vararg function that level `i` called leaves below it:
    /// PUC moves the function above its arguments (`luaT_adjustvarargs`),
    /// and the caller's level ends at the moved one. Its old slot, the
    /// fixed parameters it erased and the extra arguments stay there. 5.5's
    /// `...t` keeps the function where it was called.
    fn vararg_left(&self, ts: &ThreadStack, i: usize) -> Vec<Value> {
        let Some(DbgKind::Lua(fi)) = i.checked_sub(1).map(|j| ts.levels[j]) else {
            return vec![];
        };
        let f = ts.lua(fi);
        let p = f.closure.proto;
        let np = u32::from(p.num_params);
        let vatab = || {
            self.version() == LuaVersion::Lua55
                && p.code
                    .first()
                    .is_some_and(|ins| ins.op() == Op::GetVarg && ins.a() == np)
        };
        if self.version() < LuaVersion::Lua54 || !p.is_vararg || vatab() {
            return vec![];
        }
        let mut left = vec![ts.stack[f.func_slot as usize]];
        left.resize(1 + np as usize, Value::Nil);
        left.extend_from_slice(&ts.stack[(f.base - f.n_varargs) as usize..f.base as usize]);
        left
    }
}
