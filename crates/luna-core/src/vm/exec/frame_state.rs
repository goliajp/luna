//! Small accessors for the running frame: registers, pc, upvalues,
//! results and the per-frame closure and marker slots.

use super::*;

impl Vm {
    /// Re-sync `frames_top` after a bulk `frames: Vec`
    /// swap (take_ctx, put_ctx, load_coro_ctx). Must be called after
    /// the Vec replacement to keep the shadow valid.
    #[inline(always)]
    pub(super) fn frames_resync(&mut self) {
        // a thread switch swaps in that thread's hook
        self.trap = true;
        // Debug-only — see `frames_push_sync` comment.
        #[cfg(debug_assertions)]
        {
            self.frames_top = self.frames.len() as u32;
        }
    }

    // ====================================================================
    // Stack-inline frame metadata accessors (unused).
    //
    // These methods read/write the LJ_FR2 marker slots at `stack[base-2]`
    // (closure GCRef) and `stack[base-1]` (FrameMarker as i64). No call
    // site uses them yet.
    //
    // Preconditions (debug-asserted):
    // - base >= 2 (slots base-2 and base-1 must exist below the frame)
    // - self.stack.len() > base + max_stack (caller has grown stack)
    // - For Lua frames, stack[base-2] holds Value::Closure(cl)
    // - For Lua frames, stack[base-1] holds Value::Int(marker.to_raw())
    //
    // No release-build cost when unused (LTO strips dead methods).
    // ====================================================================

    /// Write a Lua frame's closure pointer into `stack[base-2]`.
    /// The caller must ensure `base >= 2` and the slot is within the
    /// stack's allocated range.
    #[inline]
    #[allow(dead_code)] // no consumer yet
    pub(super) fn write_frame_closure(&mut self, base: u32, cl: crate::runtime::Gc<LuaClosure>) {
        debug_assert!(
            base >= 2,
            "frame closure slot needs base >= 2; got {}",
            base
        );
        let idx = (base - 2) as usize;
        debug_assert!(idx < self.stack.len(), "stack[base-2] out of range");
        self.stack[idx] = Value::Closure(cl);
    }

    /// Read a Lua frame's closure pointer from `stack[base-2]`.
    /// Returns `None` if the slot doesn't hold a closure (caller is
    /// expected to treat that as a corrupt frame).
    ///
    /// Uses the [`Value::tag_byte`] fast-path
    /// to avoid the enum-match cost on the hot path. Tag check via
    /// 1-byte load + branch + `as_closure_unchecked` payload load.
    #[inline]
    #[allow(dead_code)]
    pub(super) fn read_frame_closure(&self, base: u32) -> Option<crate::runtime::Gc<LuaClosure>> {
        debug_assert!(base >= 2);
        let v = self.stack.get((base - 2) as usize)?;
        if v.tag_byte() == crate::runtime::value::tag::CLOSURE {
            // SAFETY: tag byte just verified == CLOSURE.
            Some(unsafe { v.as_closure_unchecked() })
        } else {
            None
        }
    }

    /// Write a packed [`FrameMarker`] into `stack[base-1]`. The marker
    /// encodes the frame kind (Lua / Cont) + PC-or-delta payload.
    /// Stored as `Value::Int(marker.to_raw())` so it round-trips
    /// cleanly through the value stack without losing bits.
    #[inline]
    #[allow(dead_code)]
    pub(super) fn write_frame_marker(
        &mut self,
        base: u32,
        marker: crate::runtime::frame_marker::FrameMarker,
    ) {
        debug_assert!(base >= 1, "frame marker slot needs base >= 1; got {}", base);
        let idx = (base - 1) as usize;
        debug_assert!(idx < self.stack.len(), "stack[base-1] out of range");
        self.stack[idx] = Value::Int(marker.to_raw());
    }

    /// Read a packed [`FrameMarker`] from `stack[base-1]`. Returns
    /// `None` if the slot isn't a `Value::Int` (caller treats as a
    /// corrupt frame); the kind tag itself may still be invalid, in
    /// which case [`FrameMarker::kind`] returns `None` on the result.
    ///
    /// Uses the [`Value::tag_byte`] fast-path
    /// for the tag check + `as_int_unchecked` for the payload load.
    #[inline]
    #[allow(dead_code)]
    pub(super) fn read_frame_marker(
        &self,
        base: u32,
    ) -> Option<crate::runtime::frame_marker::FrameMarker> {
        debug_assert!(base >= 1);
        let v = self.stack.get((base - 1) as usize)?;
        if v.tag_byte() == crate::runtime::value::tag::INT {
            // SAFETY: tag byte just verified == INT.
            Some(crate::runtime::frame_marker::FrameMarker::from_raw(
                unsafe { v.as_int_unchecked() },
            ))
        } else {
            None
        }
    }

    /// The running (top) Lua frame. The interpreter only reads this while a Lua
    /// frame is on top — a continuation frame is never the running frame (it is
    /// consumed the instant the call it protects unwinds onto it).
    #[inline]
    pub(super) fn top_frame(&self) -> &Frame {
        self.frames
            .last()
            .and_then(CallFrame::lua)
            .expect("running Lua frame")
    }

    #[inline]
    pub(super) fn top_frame_mut(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .and_then(CallFrame::lua_mut)
            .expect("running Lua frame")
    }

    /// Pad/announce results sitting at func_slot. Results past `wanted`
    /// are cleared; nothing else is: values left higher up by the call are
    /// dead and stay safe to mark (see `clear_dead_stack`), as with PUC's
    /// `moveresults`.
    #[inline]
    pub(crate) fn finish_results(&mut self, func_slot: u32, nret: u32, wanted: i32) {
        if wanted < 0 {
            self.top = func_slot + nret;
            return;
        }
        let wanted = wanted as u32;
        let new_top = func_slot + wanted;
        if nret < wanted {
            self.pad_results(func_slot + nret, new_top);
        } else if nret > wanted {
            self.stack[new_top as usize..(func_slot + nret) as usize].fill(Value::Nil);
        }
        self.top = new_top;
    }

    /// Grow the stack to `need` nil slots (`need` > its length), ending
    /// the process when the allocation fails. Out of line, and on the Vm
    /// rather than the vector, so the hot callers keep only the length test
    /// and never take the stack's address.
    #[cold]
    #[inline(never)]
    pub(crate) fn grow_stack_or_abort(&mut self, need: usize) {
        self.stack.resize_or_abort(need, Value::Nil);
    }

    /// Nil the missing results `[from, to)`.
    pub(super) fn pad_results(&mut self, from: u32, to: u32) {
        if self.stack.len() < to as usize {
            self.grow_stack_or_abort(to as usize);
        }
        self.stack[from as usize..to as usize].fill(Value::Nil);
    }

    /// Current Lua call-frame depth (read-only).
    /// Used by `EvalFuture` on the bootstrap poll to compute the
    /// `entry_depth` it will pass to subsequent resume slices.
    pub(crate) fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub(super) fn take_results(&mut self, func_slot: u32) -> Vec<Value> {
        let nret = self.top - func_slot;
        let out = self.stack[func_slot as usize..(func_slot + nret) as usize].to_vec();
        self.stack.truncate(func_slot as usize);
        self.top = func_slot;
        out
    }

    // ---- open upvalues ----

    #[doc(hidden)]
    pub fn find_or_create_upval(&mut self, slot: u32) -> Gc<Upvalue> {
        match self.open_upvals.binary_search_by_key(&slot, |&(s, _)| s) {
            Ok(i) => self.open_upvals[i].1,
            Err(i) => {
                let uv = self.heap.new_upvalue(UpvalState::Open {
                    slot,
                    thread: self.current,
                });
                self.open_upvals.insert_or_abort(i, (slot, uv));
                uv
            }
        }
    }

    pub(crate) fn close_from(&mut self, slot: u32) {
        while let Some(&(s, uv)) = self.open_upvals.last() {
            if s < slot {
                break;
            }
            let v = self.stack[s as usize];
            // SAFETY: `uv` is an entry of `open_upvals`, which the collector marks as extra roots, so it is alive; no reference into the cell is live, and the borrow covers one call
            unsafe { uv.as_mut() }.set_closed(v);
            self.heap.barrier_forward(uv, v);
            self.open_upvals.pop();
        }
    }

    #[doc(hidden)]
    pub fn upval_get(&self, cl: Gc<LuaClosure>, idx: u32) -> Value {
        match cl.upvals()[idx as usize].state() {
            UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        }
    }

    pub(super) fn upval_set(&mut self, cl: Gc<LuaClosure>, idx: u32, v: Value) {
        let uv = cl.upvals()[idx as usize];
        match uv.state() {
            UpvalState::Open { slot, thread } => self.write_slot(slot, thread, v),
            UpvalState::Closed(_) => {
                // SAFETY: `uv` is an upvalue of `cl`, the running closure its frame keeps alive; the `state()` copy above has ended, so no reference into the cell is live, and the borrow covers one call
                unsafe { uv.as_mut() }.set_closed(v);
                // forward barrier: a closed upvalue is single-slot, so the
                // forward variant is cheaper than barrier_back (PUC uses
                // `luaC_barrier_` for upvalues; `luaC_barrierback_` for
                // tables / threads).
                self.heap.barrier_forward(uv, v);
            }
        }
    }

    // ---- register / error helpers ----

    #[inline(always)]
    pub(super) fn r(&self, base: u32, i: u32) -> Value {
        // SAFETY: the compiler reserves `proto.max_stack` slots above `base`
        // at frame entry (`push_frame` sizes the stack up to base + max_stack),
        // and every bytecode-generated reference falls within `[0, max_stack)`.
        // PUC's vmfetch uses raw `R(A)` (`s2v(L->base + A)`) for the same
        // reason. The bounds check would re-validate this invariant on every
        // op — the dispatch hot path can't afford it.
        unsafe { *self.stack.get_unchecked((base + i) as usize) }
    }

    #[inline(always)]
    pub(super) fn set_r(&mut self, base: u32, i: u32, v: Value) {
        // SAFETY: see `r` — `base + i < base + max_stack <= stack.len()` by
        // frame-entry contract.
        unsafe {
            *self.stack.get_unchecked_mut((base + i) as usize) = v;
        }
    }

    #[inline(always)]
    pub(super) fn pc_of_top(&self) -> u32 {
        self.top_frame().pc
    }

    #[inline(always)]
    pub(super) fn bump_pc(&mut self) {
        // Inline `top_frame_mut`: top is guaranteed Lua (continuation frames
        // drained at dispatch loop head). Avoids the and_then/lua_mut Option
        // layers — bump_pc fires per Jmp / cond_skip miss, so the savings add
        // up over `fib_28`'s ~500k jumps.
        // SAFETY: this runs inside an op of the running Lua frame, which is the top of `frames` (the loop head drains continuation frames before dispatching), so `frames` is not empty
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => f.pc += 1,
            _ => unreachable!("Cont frame at bump_pc"),
        }
    }

    #[inline(always)]
    pub(super) fn add_pc(&mut self, d: i32) {
        // SAFETY: as in `bump_pc`: the running Lua frame is the top of `frames`, so it is not empty
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => f.pc = (f.pc as i64 + d as i64) as u32,
            _ => unreachable!("Cont frame at add_pc"),
        }
    }

    /// PUC conditional-skip convention: the JMP that follows is executed when
    /// `cond == k`; otherwise it is skipped.
    #[inline(always)]
    pub(super) fn cond_skip(&mut self, cond: bool, k: bool) {
        if cond != k {
            self.bump_pc();
        }
    }
}
