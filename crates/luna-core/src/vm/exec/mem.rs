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
        // the blocks kept for reuse go back first, collection or not: the
        // retry that follows then fails only where it would without them
        self.heap.mem().ctx().release_kept();
        if !self.emergency_allowed() {
            return false;
        }
        #[cfg(feature = "gc-trace")]
        self.heap.trace.line(format_args!("X"));
        // what is above PUC's `L->top` at the allocation is dead
        let top = self.alloc_top();
        let live_top = std::mem::replace(&mut self.gc_top, top);
        let lo = (top as usize).min(self.stack.len());
        self.stack[lo..].fill(Value::Nil);
        self.clear_saved_main();
        // what the collection frees is counted, also in the middle of what
        // luna makes without counting (a load)
        let counting = self.heap.count_all();
        let young = self.heap.young_len();
        self.gc_hold_threads();
        self.jit_forget_ghosts();
        self.name_cache = Default::default();
        if self.heap.gc_phase_is_propagate() {
            // finish the cycle in progress: what it finds dead goes; the
            // objects made since the last safe point are held as young
            let mut m = self.heap.loan_marker();
            self.mark_roots(&mut m, true);
            self.heap.mark_young(&mut m, young);
            self.heap.stash_marker(m);
            self.watch_errors();
            self.lend_open_lists();
            self.heap.gc_finish_atomic();
            self.forget_dead_errors();
            self.heap.gc_finish_sweep();
        } else {
            self.heap.gc_finish_sweep();
            self.gc_count_marked();
            let mut m = self.heap.gc_begin_full();
            self.mark_roots(&mut m, false);
            self.heap.mark_young(&mut m, young);
            self.watch_errors();
            // 5.1 and 5.2: the dead open upvalues come off their lists
            self.lend_open_lists();
            self.heap.gc_end_full(m);
            self.forget_dead_errors();
        }
        // 5.2's sweep frees each thread's unused `CallInfo`s
        if self.version == LuaVersion::Lua52 {
            self.emergency_shrinks_threads();
        }
        self.heap.keep_young(young);
        self.heap.release_pool();
        self.gc_top = live_top;
        self.heap.puc.pace.fin_num = 1;
        self.gc_full_pause();
        self.heap.restore_counting(counting);
        #[cfg(feature = "gc-trace")]
        self.trace_full_end();
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
            Ok(v) => self.refused_growth().map(|()| v),
            Err(o) => self.retry_oom_slow(o, f),
        }
    }

    /// The rest of [`Vm::retry_oom`] once `f` has failed with `o`: an
    /// emergency collection and one more run, or the memory error.
    #[cold]
    #[inline(never)]
    pub(crate) fn retry_oom_slow<T>(
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
    #[inline(always)]
    pub(crate) fn grow_stack(&mut self, need: usize) -> Result<(), LuaError> {
        if self.stack.len() >= need {
            return Ok(());
        }
        self.grow_stack_slow(need)
    }

    #[cold]
    #[inline(never)]
    fn grow_stack_slow(&mut self, need: usize) -> Result<(), LuaError> {
        self.retry_oom(|vm| vm.stack.resize(need, Value::Nil))
    }

    /// Enter native `act` on the running-natives chain, collecting in an
    /// emergency when the chain cannot grow. Its closure and arguments are
    /// on the stack, so the collection keeps them.
    #[inline(always)]
    pub(crate) fn push_native(
        &mut self,
        act: crate::vm::callstack::NativeAct,
    ) -> Result<(), LuaError> {
        // 5.1's C function takes a `CallInfo` of its array too
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        if self.running_natives.len() == self.running_natives.capacity() {
            self.native_room()?;
        }
        self.running_natives.push_or_abort(act);
        self.note_native_level();
        Ok(())
    }

    /// [`Vm::push_native`] for the fast loop's native calls, with no
    /// second room test.
    #[inline(always)]
    pub(crate) fn push_native_fast(
        &mut self,
        act: crate::vm::callstack::NativeAct,
    ) -> Result<(), LuaError> {
        // 5.1's C function takes a `CallInfo` of its array too
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        if self.running_natives.len() == self.running_natives.capacity() {
            self.native_room()?;
        }
        // SAFETY: there is room: there was, or `native_room` made it
        unsafe { self.running_natives.push_in_room(act) };
        self.note_native_level();
        Ok(())
    }

    /// Room for one more running native; out of line, and taking nothing
    /// the caller has to build first.
    #[cold]
    #[inline(never)]
    fn native_room(&mut self) -> Result<(), LuaError> {
        self.retry_oom(|vm| vm.running_natives.reserve(1))
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
            Ok(v) => self.refused_table().map(|()| v),
            r => r,
        }
    }
}
