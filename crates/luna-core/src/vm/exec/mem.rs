//! Allocation failures: the emergency collection and the allocation
//! helpers the interpreter and the libraries use.
//!
//! An allocation that fails is retried once after an emergency collection,
//! as PUC's `luaM_realloc_` does from 5.2 on, then raised as a memory error.
//! The heap itself never collects: only these helpers, called where the
//! VM holds no reference into a heap object, run the emergency collection.
//! Values the running operation holds only in Rust locals are covered by
//! rooting every object allocated since the last safe point and the whole
//! running stack.

use super::*;
use crate::runtime::LuaStr;
use crate::runtime::mem::{LVec, Oom};
use crate::runtime::table::TableError;

impl Vm {
    /// Whether an allocation that failed may run an emergency collection
    /// and try again: never in 5.1; in 5.2 only while the collector runs
    /// (`gcrunning`); from 5.3 on once the state is built. Never while
    /// finalizers run, which the collector is not reentrant for.
    fn emergency_allowed(&self) -> bool {
        if !self.state_complete || self.gc_finalizing {
            return false;
        }
        match self.version {
            LuaVersion::Lua51 => false,
            LuaVersion::Lua52 => !self.heap.gc_is_stopped(),
            _ => true,
        }
    }

    /// PUC `luaC_fullgc(L, 1)` from an allocation that failed: collect what
    /// can be collected without running finalizers (they stay queued for
    /// the next safe point). `false` when no collection may run now.
    #[cold]
    #[inline(never)]
    pub(crate) fn emergency_collect(&mut self) -> bool {
        if !self.emergency_allowed() {
            return false;
        }
        // the running operation's registers may lie above the last safe
        // point's live top
        let live_top = std::mem::replace(&mut self.gc_top, self.stack.len() as u32);
        if self.heap.gc_phase_is_propagate() {
            // finish the cycle in progress: what it finds dead goes, and
            // what was born during it stays until the next cycle
            let mut m = self.heap.loan_marker();
            self.mark_roots(&mut m);
            self.heap.mark_young(&mut m);
            self.heap.stash_marker(m);
            self.heap.gc_finish_atomic();
            self.heap.gc_finish_sweep();
        } else {
            self.heap.gc_finish_sweep();
            let mut m = self.heap.gc_begin_full();
            self.mark_roots(&mut m);
            self.heap.mark_young(&mut m);
            self.heap.gc_end_full(m);
        }
        self.heap.release_pool();
        self.gc_top = live_top;
        self.heap.rearm_gc_pause(self.gc_pause);
        true
    }

    /// Run `f`; when it fails for memory, run an emergency collection and
    /// run it once more. A failure that remains is a memory error.
    #[inline(always)]
    pub(crate) fn retry_oom<T>(
        &mut self,
        mut f: impl FnMut(&mut Vm) -> Result<T, Oom>,
    ) -> Result<T, LuaError> {
        match f(self) {
            Ok(v) => Ok(v),
            Err(o) => self.retry_oom_slow(o, f),
        }
    }

    #[cold]
    #[inline(never)]
    fn retry_oom_slow<T>(
        &mut self,
        o: Oom,
        mut f: impl FnMut(&mut Vm) -> Result<T, Oom>,
    ) -> Result<T, LuaError> {
        if !self.emergency_collect() {
            return Err(o.into());
        }
        f(self).map_err(LuaError::from)
    }

    /// Grow the running stack to at least `need` slots (new ones nil),
    /// collecting in an emergency when memory runs out (PUC
    /// `luaD_growstack`).
    #[cold]
    #[inline(never)]
    pub(crate) fn grow_stack(&mut self, need: usize) -> Result<(), LuaError> {
        if self.stack.len() >= need {
            return Ok(());
        }
        self.retry_oom(|vm| vm.stack.resize(need, Value::Nil))
    }

    /// A string with `bytes`, collecting in an emergency when memory runs
    /// out.
    #[inline]
    pub(crate) fn alloc_str(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, LuaError> {
        self.retry_oom(|vm| vm.heap.try_intern(bytes))
    }

    /// [`Vm::alloc_str`] as a value.
    #[inline]
    pub(crate) fn str_value(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        self.alloc_str(bytes).map(Value::Str)
    }

    /// A string formatted from `args`, composed in a block of the
    /// allocation context, collecting in an emergency when memory runs out.
    pub(crate) fn fmt_str(
        &mut self,
        args: std::fmt::Arguments<'_>,
    ) -> Result<Gc<LuaStr>, LuaError> {
        if let Some(s) = args.as_str() {
            return self.alloc_str(s.as_bytes());
        }
        let buf = self.fmt_bytes(args)?;
        self.alloc_str(&buf)
    }

    /// The bytes formatted from `args`, in a block of the allocation
    /// context.
    pub(crate) fn fmt_bytes(
        &mut self,
        args: std::fmt::Arguments<'_>,
    ) -> Result<LVec<u8>, LuaError> {
        struct W<'a>(&'a mut LVec<u8>, Option<Oom>);
        impl std::fmt::Write for W<'_> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.0.extend_from_slice(s.as_bytes()).map_err(|o| {
                    self.1 = Some(o);
                    std::fmt::Error
                })
            }
        }
        let mut buf = LVec::new(self.heap.mem());
        let mut w = W(&mut buf, None);
        if std::fmt::write(&mut w, args).is_err() {
            return Err(match w.1 {
                Some(o) => o.into(),
                None => self.plain_err("invalid format"),
            });
        }
        Ok(buf)
    }

    /// An empty table, collecting in an emergency when memory runs out.
    #[inline]
    pub(crate) fn alloc_table(&mut self) -> Result<Gc<Table>, LuaError> {
        self.retry_oom(|vm| vm.heap.try_new_table())
    }

    /// A table with room for `narr` array items and `nrec` others (PUC
    /// `luaH_new` + `luaH_resize`), collecting in an emergency when memory
    /// runs out.
    pub(crate) fn alloc_table_sized(
        &mut self,
        narr: usize,
        nrec: usize,
    ) -> Result<Gc<Table>, LuaError> {
        let t = self.alloc_table()?;
        if narr > 0 || nrec > 0 {
            // the new table is young, so an emergency collection keeps it
            self.table_op(|vm| {
                // SAFETY: `t` was allocated above and is held only by this
                // local; the borrow covers the resizes, which do not collect
                let tm = unsafe { t.as_mut() };
                if narr > 0 {
                    tm.ensure_array(&mut vm.heap, narr)?;
                }
                if nrec > 0 {
                    tm.ensure_hash(&mut vm.heap, nrec)?;
                }
                Ok(())
            })?;
        }
        Ok(t)
    }

    /// Run a table operation `f`; when it fails for memory, run an emergency
    /// collection and run it once more. Table operations leave the table as
    /// it was when they fail, so running one again is safe. Any other
    /// table error comes back as is.
    #[inline(always)]
    pub(crate) fn table_op<T>(
        &mut self,
        mut f: impl FnMut(&mut Vm) -> Result<T, TableError>,
    ) -> Result<T, TableError> {
        match f(self) {
            Err(TableError::Memory(_)) if self.emergency_collect() => f(self),
            r => r,
        }
    }
}
