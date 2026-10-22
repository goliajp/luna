//! The objects of PUC's that luna's libraries make for the count (the
//! pieces of a formatted string, a concatenation's, records of objects
//! luna has none of), each collecting in an emergency when memory runs out
//! and made again, as every allocation of PUC's is.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::heap::PucKind;
use crate::runtime::puc_fmt::FmtArg;

impl Vm {
    /// PUC's `L->top` where an allocation is asked for: the running
    /// native's top, else what the running Lua function's instruction has
    /// set it to below the frame's end, else the whole frame. Worked out
    /// here, when memory has run out, so the instructions pay nothing.
    pub(crate) fn alloc_top(&self) -> u32 {
        if let Some(top) = self.native_top() {
            return top;
        }
        let Some(crate::runtime::function::CallFrame::Lua(f)) = self.frames.last() else {
            return self.lua_window_end();
        };
        let p = &f.closure.proto;
        // a call before its frame is made (`luaD_precall`'s stack and
        // `CallInfo`): the arguments, the missing ones nil already
        if f.pc == 0 {
            return if p.is_vararg {
                self.lua_window_end()
            } else {
                f.base + u32::from(p.num_params)
            };
        }
        let at = |i: u32| p.code.get(f.pc.wrapping_sub(i) as usize).copied();
        let open_setlist = |i: Option<crate::vm::isa::Inst>| {
            i.is_some_and(|i| i.op() == crate::vm::isa::Op::SetList && i.b() == 0)
        };
        // `CONCAT` folds its operands down to `Vm::top`; an open `SETLIST`
        // stores the values up to it (its extra argument read already)
        match at(1) {
            Some(i) if i.op() == crate::vm::isa::Op::Concat => self.top,
            i if open_setlist(i) || open_setlist(at(2)) => self.top,
            _ => self.lua_window_end(),
        }
    }

    /// `lua_pushfstring`'s string of `fmt` and `args`.
    pub(crate) fn pushfstring(
        &mut self,
        fmt: &[u8],
        args: &[FmtArg<'_>],
    ) -> Result<Gc<LuaStr>, LuaError> {
        self.retry_oom(|vm| vm.heap.pushfstring(fmt, args))
    }

    /// A record of an object of `size` bytes PUC makes and luna does not.
    pub(crate) fn ghost(&mut self, kind: PucKind, size: usize) -> Result<(), LuaError> {
        self.retry_oom(|vm| vm.heap.acct_ghost(kind, size))
    }

    /// `lua_concat` of the last `total` values of `st`, which the collector
    /// does not see: they are held while it collects.
    pub(crate) fn concat_pieces(
        &mut self,
        st: &mut Vec<Value>,
        total: usize,
    ) -> Result<(), LuaError> {
        let len = st.len();
        let Err(o) = self.heap.puc_concat(st, total) else {
            return Ok(());
        };
        let base = self.lib_roots.len();
        self.lib_roots.extend_from_slice(st);
        let collected = self.emergency_collect();
        self.lib_roots.truncate(base);
        if !collected {
            return Err(o.into());
        }
        // what was joined before the failure stays joined
        let left = total - (len - st.len());
        Ok(self.heap.puc_concat(st, left)?)
    }

    /// `lua_concat` of the last `total` values of `lib_roots`.
    pub(crate) fn concat_roots(&mut self, total: usize) -> Result<(), LuaError> {
        let mut st = std::mem::take(&mut self.lib_roots);
        let r = self.concat_pieces(&mut st, total);
        self.lib_roots = st;
        r
    }
}
