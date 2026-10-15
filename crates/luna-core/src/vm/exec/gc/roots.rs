//! The collector's roots.

use super::*;

impl Vm {
    /// Mark the GC roots in `m`: first-class `Value` roots plus bare-object
    /// roots (open upvalues, which are not first-class Values). Shared by the
    /// full collector and the incremental driver so both mark the exact
    /// same live set. Marks in place, without collecting the roots first, so
    /// a collection needs no memory for them.
    pub(crate) fn mark_roots(&self, m: &mut crate::runtime::heap::Marker) {
        m.value(Value::Table(self.globals));
        for mt in self.type_mt.into_iter().flatten() {
            m.value(Value::Table(mt));
        }
        for &n in self
            .mm_names
            .iter()
            .chain(&self.heap.puc.pins)
            .chain(&self.heap.puc.load_msg)
        {
            m.value(Value::Str(n));
        }
        // Root the running thread's live registers (PUC marks [stack, top)).
        // `gc_top` is the instruction-level cursor of the last GC
        // safe-point: allocation safe-points set it via
        // `checkpoint(live_top)`, and `begin_call` raises it
        // to the callee's argument top when entering a native — PUC's
        // `L->top = func + 1 + nargs` C-call discipline. Without that
        // raise, an explicit `collectgarbage()` collected with a STALE
        // cursor from some earlier (lower) safe-point and freed its own
        // caller's register-held strings
        // (STATUS_ACCESS_VIOLATION on Windows / ASAN heap-use-after-free
        // on Linux). Values stranded above the cursor stay
        // excluded so weak-table entries are not spuriously pinned
        // (gc.lua:544 suspended-coroutine collection).
        let live = (self.gc_top as usize).min(self.stack.len());
        for &v in self.stack[..live].iter().chain(self.jit.ssa_roots.iter()) {
            m.value(v);
        }
        for cf in &self.frames {
            match cf {
                CallFrame::Lua(f) => {
                    m.value(Value::Closure(f.closure));
                }
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Xpcall { handler, .. },
                    ..
                }) => {
                    m.value(*handler);
                }
                // a close chain's threaded error sits on the stack below its
                // handler's call, inside the live window
                CallFrame::Cont(_) => {}
            }
        }
        if let Some(e) = self.closing_err {
            m.value(e);
        }
        for &v in self.host_light.values() {
            m.value(v);
        }
        // Host roots — Lua-facade handles keep their referenced
        // values alive across calls/yields. Trace the whole vector;
        // unused slots (post-`unpin_all`) carry Value::Nil which the
        // GC ignores.
        for slot in &self.host_roots {
            // free-list slots carry Value::Nil (GC no-op)
            m.value(slot.value);
        }
        // `table.sort` and similar builtins stash their working
        // `Vec<Value>` here so a `collectgarbage()` invoked inside the
        // comparator callback doesn't free strings/tables snapshotted
        // off the live table (sort.lua's `load(..)(); collectgarbage()`
        // compare regression).
        for &v in self.sort_scratch.iter().flatten().chain(&self.lib_roots) {
            m.value(v);
        }
        // The running-natives chain holds Gc<NativeClosure>s
        // mid-execution. Without rooting them here, a `collectgarbage()`
        // invoked inside the running native (sort.lua's `load(..)();
        // collectgarbage()` compare callback regression) sweeps the
        // closure that's actively executing, leaving `nc.upvals`
        // dangling and the Rust local `nc` pointing at recycled memory
        // — the SIGSEGV pops on the very next field access or pop.
        for a in &self.running_natives {
            m.value(Value::Native(a.nc));
        }
        // the running thread's debug hook (suspended threads root theirs via
        // Coro::trace / the main_ctx sweep below)
        if let Some(h) = self.hook.func {
            m.value(h);
        }
        // the running coroutine (its saved-context fields live in the VM, but
        // the object itself + its resumer chain must stay reachable)
        if let Some(co) = self.current {
            m.value(Value::Coro(co));
        }
        if let Some(mc) = self.main_coro {
            m.value(Value::Coro(mc));
        }
        // debug.getregistry() and io library state
        if let Some(r) = self.registry {
            m.value(Value::Table(r));
        }
        if let Some(mt) = self.file_mt {
            m.value(Value::Table(mt));
        }
        if let Some(f) = self.io_input {
            m.value(Value::Userdata(f));
        }
        if let Some(f) = self.io_output {
            m.value(Value::Userdata(f));
        }
        if let Some(f) = self.io_stdin {
            m.value(Value::Userdata(f));
        }
        // the main thread's saved context while a coroutine runs
        if let Some(mc) = &self.main_ctx {
            for &v in mc.stack.iter() {
                m.value(v);
            }
            if let Some(h) = mc.hook.func {
                m.value(h);
            }
            for cf in mc.frames.iter() {
                match cf {
                    CallFrame::Lua(f) => {
                        m.value(Value::Closure(f.closure));
                    }
                    CallFrame::Cont(NativeCont {
                        kind: ContKind::Xpcall { handler, .. },
                        ..
                    }) => {
                        m.value(*handler);
                    }
                    CallFrame::Cont(_) => {}
                }
            }
        }
        for &(_, uv) in self.open_upvals.iter() {
            m.mark(uv);
        }
        if let Some(mc) = &self.main_ctx {
            for &(_, uv) in mc.open_upvals.iter() {
                m.mark(uv);
            }
        }
    }
}
