//! The objects of PUC's that luna's libraries make for the count (the
//! pieces of a formatted string, a concatenation's, records of objects
//! luna has none of), each collecting in an emergency when memory runs out
//! and made again, as every allocation of PUC's is.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::heap::PucKind;
use crate::runtime::puc_fmt::FmtArg;

impl Vm {
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
