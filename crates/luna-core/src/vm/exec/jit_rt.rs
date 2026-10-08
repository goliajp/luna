//! Runtime entry points the JIT-compiled code calls back into.

use super::*;

impl Vm {
    /// Last live Lua frame (the trace head's frame at
    /// dispatch time). The frame-materialization helper reads `.base`
    /// to compute offsets for each inlined frame's window.
    #[doc(hidden)]
    pub fn jit_last_lua_frame(&self) -> Option<Frame> {
        match self.frames.last() {
            Some(CallFrame::Lua(f)) => Some(*f),
            _ => None,
        }
    }

    /// Read-only borrow of the current call
    /// stack, for the [`crate::vm::inspect`] pure-read accessors used
    /// by `luna-tools` (`luna-profile`'s sampler walks this from
    /// inside a `Count` hook). Sibling-module scope: not part of the
    /// public embedder surface, but `inspect::frames_for_profile` is.
    #[doc(hidden)]
    pub(in crate::vm) fn inspect_frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// Ensure the value stack covers indices
    /// `[0..need)`. Extends with Nil if shorter. Called by the
    /// frame-materialization helper before pushing an inlined frame
    /// whose register window may exceed the current stack length.
    #[doc(hidden)]
    pub fn jit_ensure_stack(&mut self, need: usize) {
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
    }

    /// Trace JIT path for `Op::Close A`. Predicts whether
    /// `__close` handlers would run (any active tbc slot ≥ from
    /// holding a non-nil/false Value); if so, returns 1 without doing
    /// anything and the trace side-exits at the op, so the interpreter
    /// runs the handlers. Otherwise performs the safe part of close —
    /// `close_from(from)` to close open upvals + drop any drained tbc
    /// entries ≥ from — and returns 0.
    ///
    /// Returns are i64-shaped so the cranelift import sig stays
    /// trivial (i64 → i64 mapping).
    #[doc(hidden)]
    pub fn jit_op_close(&mut self, start_offset: u32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return 1;
        };
        let from = f.base + start_offset;
        let has_handler = self.tbc.iter().any(|&s| {
            s >= from && {
                let v = self.stack[s as usize];
                !matches!(v, Value::Nil | Value::Bool(false))
            }
        });
        if has_handler {
            self.jit.counters.deopt += 1;
            return 1;
        }
        self.close_from(from);
        // Drain any tbc entries ≥ from (they're nil/false stubs the
        // interpreter's drive_close would have skipped silently).
        while let Some(&s) = self.tbc.last() {
            if s < from {
                break;
            }
            self.tbc.pop();
        }
        0
    }

    /// Spill the trace's current value for a register to
    /// the underlying `vm.stack[base + slot_offset]`. Required before
    /// an `Op::Closure` whose inner proto has an `in_stack: true`
    /// upval at `slot_offset` — the helper's `find_or_create_upval`
    /// captures a live pointer to `vm.stack[base + slot_offset]`,
    /// which must hold the right value at call time (trace IR's
    /// Variable hasn't yet been written back).
    ///
    /// `slot_offset` is the caller-frame register index (depth=0
    /// only — depth>0 Closure is not supported); `v` is the trace
    /// register's value, which the helper packs from its tag and
    /// payload.
    #[doc(hidden)]
    pub fn jit_spill_stack(&mut self, slot_offset: u32, v: Value) {
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err =
                Some(self.rt_err("JIT spill: no Lua frame on jit_last_lua_frame()"));
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if self.stack.len() <= idx {
            self.grow_stack_or_abort(idx + 1);
        }
        self.stack[idx] = v;
    }

    /// `vm.stack[base + slot_offset]` of the trace's head frame, or
    /// `None` when there is no Lua frame or the slot is past the
    /// stack. The trace JIT's Op::Concat body emit refreshes the
    /// payload of a slot whose `RegKind` is `Unset` (no compile-time
    /// tag info; commonly `Str` slots which the trace doesn't model)
    /// through it, keeping the tag the interpreter left there.
    #[doc(hidden)]
    pub fn jit_stack_slot_mut(&mut self, slot_offset: u32) -> Option<&mut Value> {
        let f = self.jit_last_lua_frame()?;
        let idx = (f.base as usize) + (slot_offset as usize);
        self.stack.get_mut(idx)
    }

    /// Trace JIT path for `Op::Concat A B`.
    ///
    /// Mirrors the interp arm (`run_frame_op`): `self.top =
    /// base + a + n; concat_run(base + a)`. Result lands at
    /// `vm.stack[base + a]`. Returns `0` on success, `-1` when the
    /// interpreter must do it (any error from `concat_run` OR
    /// detection that the metamethod path was taken — `concat_run`
    /// returns `Ok(())` after `begin_meta_call` which has pushed a Lua
    /// frame the trace can't safely continue past); the trace then
    /// side-exits at the op and the interpreter redoes it, raising the
    /// error or calling `__concat` itself.
    ///
    /// The frame-push detection uses `pre/post frames.len()` and
    /// unwinds any pushed frames first, so the exit sees a clean stack.
    #[doc(hidden)]
    pub fn jit_op_concat(&mut self, slot_offset: u32, n: i32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return -1;
        };
        let abs_a = f.base + slot_offset;
        self.top = abs_a + n as u32;
        let pre_frames = self.frames.len();
        let result = self.concat_run(abs_a);
        let post_frames = self.frames.len();
        // Frame-push = metamethod path taken (begin_meta_call pushed
        // a Lua frame). The trace can't continue past it; unwind +
        // deopt so interp redoes Op::Concat in the slow path.
        while self.frames.len() > pre_frames {
            self.pop_frame();
        }
        if result.is_err() || post_frames > pre_frames {
            self.jit.counters.deopt += 1;
            return -1;
        }
        0
    }

    /// Pop a reusable `Vec<u8>` from the JIT accumulator buffer
    /// pool, or allocate one when the pool is empty. The trace keeps
    /// it (as the boxed pointer the helper leaks) in a stack slot
    /// through the loop and appends each piece to it.
    #[doc(hidden)]
    pub fn jit_str_buf_acquire(&mut self) -> Box<Vec<u8>> {
        Box::new(self.jit.str_buf_pool.pop().unwrap_or_default())
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend Vec.
    #[doc(hidden)]
    #[allow(clippy::boxed_local)] // the trace held the buffer boxed; it comes back that way
    pub fn jit_str_buf_release(&mut self, mut buf: Box<Vec<u8>>) {
        buf.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(*buf);
        }
        // Else: drop the buffer.
    }

    /// Append a piece's bytes to an accumulator buffer.
    #[doc(hidden)]
    pub fn jit_str_buf_extend(&mut self, buf: &mut Vec<u8>, piece: Gc<crate::runtime::LuaStr>) {
        buf.extend_from_slice(piece.as_bytes());
    }

    /// Drain the accumulator buffer into a fresh
    /// `LuaStr` via `heap.intern`, returning the raw ptr bits for
    /// the trace to write into the accumulator slot.
    ///
    /// Returns the LuaStr ptr as i64 on success, 0 on overflow
    /// (the hard cap; the trace deopts). The buffer is left
    /// CLEAR (drained) ready for release.
    #[doc(hidden)]
    pub fn jit_str_buf_intern(&mut self, buf: &mut Vec<u8>) -> i64 {
        let bytes = std::mem::take(buf);
        // hard cap at 256KB
        if bytes.len() > 256 * 1024 {
            return 0;
        }
        let gc = self.heap.intern(&bytes);
        gc.as_ptr() as i64
    }

    /// Load the raw `i64` payload of
    /// `vm.stack[base + slot_offset]` for the active trace's head
    /// Lua frame. Used to reload trace IR `Variable`s after a
    /// helper has written to `vm.stack` directly (e.g. TForCall's
    /// iter results land at `R[A+4..A+4+nvars]`).
    #[doc(hidden)]
    pub fn jit_stack_load(&mut self, slot_offset: u32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return 0;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return 0;
        }
        let v = self.stack[idx];
        let (_, raw) = v.unpack();
        // SAFETY: `raw` came from `unpack`, whose payload has all 8 bytes initialised
        unsafe { raw.zero as i64 }
    }

    /// Read the tag byte of
    /// `vm.stack[base + slot_offset]`. Used by `Op::TForLoop` emit
    /// to dispatch on the iterator's return-key tag at runtime
    /// (`raw::NIL` → loop end exit, `raw::INT` → continue, other →
    /// deopt).
    #[doc(hidden)]
    pub fn jit_stack_tag(&mut self, slot_offset: u32) -> u8 {
        let Some(f) = self.jit_last_lua_frame() else {
            return crate::runtime::value::raw::NIL;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return crate::runtime::value::raw::NIL;
        }
        self.stack[idx].unpack().0
    }

    /// Set the stack top to register `rel` of the running trace's head
    /// frame: where the interpreter would leave it after a call that
    /// returned every value, or a vararg expansion, for an op that reads
    /// it (a call or return of a variable count) and an exit before that
    /// op.
    #[doc(hidden)]
    pub fn jit_set_top(&mut self, rel: u32) {
        if let Some(f) = self.jit_last_lua_frame() {
            self.top = f.base + rel;
        }
    }

    /// Push a Lua frame onto the call stack with JIT-known metadata, for
    /// `luna_jit_trace_materialize_frames` at a trace exit inside a
    /// function the trace inlined. `nresults` is the caller's wanted count
    /// (-1 for all); a vararg `cl` has `n_varargs` extra arguments just
    /// below `base`, the function one below them, as `push_frame` leaves
    /// them. The caller has already called `jit_ensure_stack` to cover
    /// `[0..base + cl.proto.max_stack)`.
    #[doc(hidden)]
    pub fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
        n_varargs: u32,
    ) {
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc,
                func_slot: base - 1 - n_varargs,
                n_varargs,
                nresults,
                hook_oldpc: u32::MAX,
                from_c: false,
                tm: None,
                is_hook: false,
                tailcalls: 0,
                ccmt: 0,
            }),
        );
    }

    /// Upvalue `idx` of `cl`, for compiled code running a function a trace
    /// inlined, whose head frame starts at `head_base`: `None` when the
    /// upvalue is open at a slot of the running thread at or above
    /// `head_base`, a register the trace may hold only in its own
    /// registers while the stack has an older value.
    #[doc(hidden)]
    pub fn jit_upval_below(
        &self,
        cl: Gc<LuaClosure>,
        idx: u32,
        head_base: u32,
    ) -> Option<crate::runtime::Value> {
        use crate::runtime::UpvalState;
        match cl.upvals()[idx as usize].state() {
            UpvalState::Open { slot, thread }
                if slot >= head_base && self.is_current_thread(thread) =>
            {
                None
            }
            UpvalState::Open { slot, thread } => Some(self.read_slot(slot, thread)),
            UpvalState::Closed(v) => Some(v),
        }
    }

    /// `t[key]` through table-valued `__index` links (up to four, as the
    /// interpreter's fast path follows), for compiled code: `None` when a
    /// link is a function or the chain goes on, which the interpreter has
    /// to run.
    #[doc(hidden)]
    pub fn jit_index_str_tables(
        &self,
        t: Gc<Table>,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<crate::runtime::Value> {
        use crate::runtime::Value;
        let mut cur = t;
        for _ in 0..4 {
            let v = cur.get_str(key);
            if !v.is_nil() {
                return Some(v);
            }
            let Some(mt) = cur.metatable() else {
                return Some(Value::Nil);
            };
            match self.fast_tm(mt, Mm::Index) {
                Value::Nil => return Some(Value::Nil),
                Value::Table(next) => cur = next,
                _ => return None,
            }
        }
        None
    }
}
