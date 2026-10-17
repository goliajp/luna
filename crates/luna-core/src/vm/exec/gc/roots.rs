//! The collector's roots.

use super::*;

impl Vm {
    /// Mark the GC roots in `m`: first-class `Value` roots plus bare-object
    /// roots (open upvalues, which are not first-class Values). Shared by the
    /// full collector and the incremental driver so both mark the exact
    /// same live set. Marks in place, without collecting the roots first, so
    /// a collection needs no memory for them.
    /// `again`: the atomic step of a cycle whose start marked them.
    pub(crate) fn mark_roots(&self, m: &mut crate::runtime::heap::Marker, again: bool) {
        m.value(Value::Table(self.globals));
        for mt in self.type_mt.into_iter().flatten() {
            m.value(Value::Table(mt));
        }
        self.mark_root_strings(m, again);
        self.mark_running(m);
        self.mark_main_ctx(m);
    }

    /// The roots a cycle starts from, in PUC's order (`markroot`, later
    /// `restartcollection`), which decides the order everything else is
    /// traversed in: the main thread first (its values are marked when it
    /// is traversed, [`Vm::mark_thread_values`]), 5.1's globals, the
    /// registry, the basic types' metatables.
    pub(crate) fn mark_roots_restart(&self, m: &mut crate::runtime::heap::Marker) {
        self.mark_root_strings(m, false);
        if let Some(mc) = self.main_coro {
            m.value(Value::Coro(mc));
        }
        if self.version == LuaVersion::Lua51 {
            m.value(Value::Table(self.globals));
        }
        if let Some(r) = self.registry {
            m.value(Value::Table(r));
        }
        // PUC's tag order: nil, boolean, light userdata, number, string,
        // function, thread
        for slot in [0, 1, 5, 2, 3, 4, 6] {
            if let Some(mt) = self.type_mt[slot] {
                m.value(Value::Table(mt));
            }
        }
    }

    /// The values of thread `h` that the VM holds, when the step traverses
    /// it: the running thread's, or the main one's while a coroutine runs.
    /// The running thread carries what luna roots outside PUC's objects.
    pub(crate) fn mark_thread_values(
        &self,
        m: &mut crate::runtime::heap::Marker,
        h: *mut crate::runtime::heap::GcHeader,
    ) {
        let running = self.current.or(self.main_coro);
        if running.is_some_and(|c| c.as_ptr() as *mut _ == h) {
            if self.version == LuaVersion::Lua51 {
                m.value(Value::Table(self.globals));
            }
            self.mark_running(m);
        } else {
            self.mark_main_ctx(m);
        }
    }

    /// The strings luna roots: the names PUC fixes and the strings PUC's
    /// libraries keep, and the message a load may have made.
    fn mark_root_strings(&self, m: &mut crate::runtime::heap::Marker, again: bool) {
        // the root strings stay black from the cycle's start to its sweep:
        // the atomic step marks only the pins added since, and the message
        // a load may have made meanwhile
        let puc = &self.heap.puc;
        if again {
            m.mark_strings(puc.pins[puc.pins_marked.get()..].iter());
        } else {
            m.mark_strings(self.mm_names.iter());
            m.mark_strings(puc.pins.iter());
            puc.pins_marked.set(puc.pins.len());
        }
        m.mark_strings(puc.load_msg.iter());
    }

    /// The running thread's values, and the roots luna has outside PUC's
    /// objects.
    fn mark_running(&self, m: &mut crate::runtime::heap::Marker) {
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
        for u in self.live_boxes() {
            m.value(Value::Userdata(u));
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
        for &(_, uv) in self.open_upvals.iter() {
            m.mark(uv);
        }
    }

    /// The main thread's saved context while a coroutine runs.
    fn mark_main_ctx(&self, m: &mut crate::runtime::heap::Marker) {
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
        if let Some(mc) = &self.main_ctx {
            for &(_, uv) in mc.open_upvals.iter() {
                m.mark(uv);
            }
        }
    }
}
