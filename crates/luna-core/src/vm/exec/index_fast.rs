//! The table reads and writes the dispatch loop finishes itself, and the
//! hand-over to the metamethod chains for the rest.

use super::*;

impl Vm {
    /// `t[key]` without `__index` (PUC `luaV_fastget`): a raw hit on a table
    /// is the result, since `__index` is consulted only when the raw value is
    /// nil, and a miss on a table without a metatable is nil. `None` leaves
    /// the read to [`Self::index_miss`].
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn index_raw(&self, t: Value, key: Value) -> Option<Value> {
        // gc-verify builds keep every read on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t {
            let v = match key {
                Value::Str(s) => tb.get_str(s),
                Value::Int(i) => tb.get_int(i),
                k => tb.get(k),
            };
            if !v.is_nil() || tb.metatable().is_none() {
                return Some(v);
            }
        }
        None
    }

    /// A read opcode that [`Self::index_raw`] could not finish: continues the
    /// `__index` chain without repeating the raw probe.
    #[inline(never)]
    pub(super) fn index_miss(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        #[cfg(not(feature = "gc-verify"))]
        {
            if let Value::Str(s) = key {
                return self.index_str_miss(t, s, dst);
            }
            self.op_index_from(t, key, dst, matches!(t, Value::Table(_)))
        }
        #[cfg(feature = "gc-verify")]
        self.op_index(t, key, dst)
    }

    /// `t[key] := v` without `__newindex` (PUC `luaV_fastset`): overwriting a
    /// key that is present with a non-nil value never involves `__newindex`.
    /// `false` leaves the write to [`Self::newindex_miss`], having done nothing.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn newindex_raw(&mut self, t: Value, key: Value, v: Value) -> bool {
        // gc-verify builds keep every write on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            && unsafe { tb.as_mut() }.try_set_existing(key, v)
        {
            self.heap
                .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return true;
        }
        false
    }

    /// A write opcode that [`Self::newindex_raw`] could not finish: runs the
    /// `__newindex` chain without repeating that probe.
    #[inline(never)]
    pub(super) fn newindex_miss(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        let probed = cfg!(not(feature = "gc-verify")) && matches!(t, Value::Table(_));
        self.op_newindex_from(t, key, v, probed)
    }
}
