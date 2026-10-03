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
            self.stack.resize(need, Value::Nil);
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
    /// Parameters arrive as i64 from the IR: `slot_offset` is the
    /// caller-frame register index (`u32` in practice, depth=0
    /// only — depth>0 Closure is not supported); `tag` is the
    /// `crate::runtime::value::raw` byte for the slot's RegKind;
    /// `raw_bits` is the trace Variable's `use_var` payload
    /// (i64-shaped — Float is its bit-pattern, Table/Closure is the
    /// raw `Gc::as_ptr` cast).
    #[doc(hidden)]
    pub fn jit_spill_stack(&mut self, slot_offset: u32, tag: u8, raw_bits: u64) {
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err =
                Some(self.rt_err("JIT spill: no Lua frame on jit_last_lua_frame()"));
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if self.stack.len() <= idx {
            self.stack.resize(idx + 1, Value::Nil);
        }
        // SAFETY: the trace passes a register's tag with the payload it
        // holds, the shape `Value::unpack` produces; the fn is safe to call,
        // so nothing but that convention ties `raw_bits` to `tag`
        let v = unsafe {
            crate::runtime::Value::pack(tag, crate::runtime::value::RawVal { zero: raw_bits })
        };
        self.stack[idx] = v;
    }

    /// Refresh only the raw payload of
    /// `vm.stack[base + slot_offset]`, preserving its existing
    /// `Value` tag. The caller (trace JIT Op::Concat body emit)
    /// uses this when the slot's `RegKind` is `Unset` (no compile-
    /// time tag info; commonly `Str` slots which the trace doesn't
    /// model). The interp's previous execution of the same op
    /// already populated the slot with the right tag — the trace
    /// only needs to swap in its current raw value.
    #[doc(hidden)]
    pub fn jit_stack_update_raw(&mut self, slot_offset: u32, raw_bits: u64) {
        let Some(f) = self.jit_last_lua_frame() else {
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return;
        }
        let (tag, _) = self.stack[idx].unpack();
        // SAFETY: `tag` is the slot's current tag, and the trace passes the payload of a value of that type (it calls this only for a slot the interpreter last left holding the same kind of value); the fn is safe to call, so nothing but that convention ties `raw_bits` to `tag`
        self.stack[idx] = unsafe {
            crate::runtime::Value::pack(tag, crate::runtime::value::RawVal { zero: raw_bits })
        };
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
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        }
        if result.is_err() || post_frames > pre_frames {
            self.jit.counters.deopt += 1;
            return -1;
        }
        0
    }

    /// Pop a reusable `Vec<u8>` from the JIT
    /// accumulator buffer pool, returning a raw pointer. The trace
    /// fn's IR holds this pointer in a stack slot through the loop
    /// and calls `jit_str_buf_extend` per iter. If the pool is
    /// empty, allocate fresh.
    ///
    /// Safety: the returned pointer is valid until
    /// `jit_str_buf_release` is called or the Vm is dropped. The
    /// caller MUST not retain it across `enter_jit` boundaries.
    #[doc(hidden)]
    pub fn jit_str_buf_acquire(&mut self) -> *mut Vec<u8> {
        let buf = self.jit.str_buf_pool.pop().unwrap_or_default();
        // Move into a Box so the pointer is stable until release.
        Box::into_raw(Box::new(buf))
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend Vec.
    ///
    /// Safety: `buf` must have been returned by a prior
    /// `jit_str_buf_acquire` on the same Vm.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` round-trips through `Box::into_raw`; SAFETY documented below.
    pub fn jit_str_buf_release(&mut self, buf: *mut Vec<u8>) {
        if buf.is_null() {
            return;
        }
        // SAFETY: by the contract in the doc above, `buf` came from
        // `Box::into_raw` in `jit_str_buf_acquire` on this Vm and is released
        // once, so this takes back sole ownership; the signature is safe and
        // does not enforce that contract
        let mut owned = unsafe { Box::from_raw(buf) };
        owned.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(*owned);
        }
        // Else: drop the buffer.
    }

    /// Append a LuaStr's bytes to the accumulator
    /// buffer. The trace IR computes the `str_ptr` (= raw bits of
    /// the piece slot) and passes it through; we treat it as a
    /// `*mut LuaStr` and append its bytes.
    ///
    /// Returns 0 on success, -1 if the piece isn't a Str (would
    /// trip __concat metamethod path → deopt to interp).
    ///
    /// Safety: `buf` from prior `acquire`; `str_ptr` from the
    /// trace's piece slot raw bits.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` from prior `acquire`; `str_ptr` from trace piece slot; SAFETY documented below.
    pub fn jit_str_buf_extend(&mut self, buf: *mut Vec<u8>, str_ptr: i64) -> i64 {
        if buf.is_null() || str_ptr == 0 {
            return -1;
        }
        // SAFETY: `buf` is non-null and, by the contract in the doc above, came from `jit_str_buf_acquire` on this Vm and is not released yet, so it is a live boxed `Vec` that only this call uses; the signature is safe and does not enforce that contract
        let buf = unsafe { &mut *buf };
        let lua_str_ptr = str_ptr as *const crate::runtime::string::LuaStr;
        // SAFETY: `str_ptr` is non-zero and is the raw payload of a string register of the running trace, so it points at a string that register keeps alive; nothing here checks that the register holds a string (the signature is safe)
        let bytes = unsafe { crate::runtime::string::bytes_of(lua_str_ptr) };
        buf.extend_from_slice(bytes);
        0
    }

    /// Drain the accumulator buffer into a fresh
    /// `LuaStr` via `heap.intern`, returning the raw ptr bits for
    /// the trace to write into the accumulator slot.
    ///
    /// Returns the LuaStr ptr as i64 on success, 0 on overflow
    /// (the hard cap; the trace deopts).
    ///
    /// Safety: `buf` from prior `acquire`. The buffer is left
    /// CLEAR (drained) ready for `release`.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` from prior `acquire`; SAFETY documented below.
    pub fn jit_str_buf_intern(&mut self, buf: *mut Vec<u8>) -> i64 {
        if buf.is_null() {
            return 0;
        }
        // SAFETY: `buf` is non-null and, by the contract in the doc above, came from `jit_str_buf_acquire` on this Vm and is not released yet; the signature is safe and does not enforce that contract
        let buf = unsafe { &mut *buf };
        let bytes = std::mem::take(buf);
        // hard cap at 256KB
        if bytes.len() > 256 * 1024 {
            return 0;
        }
        let gc = self.heap.intern(&bytes);
        gc.as_ptr() as i64
    }

    /// Trace JIT helper for `Op::TForCall A 0 C`.
    ///
    /// Base path: copy R[A..=A+2] → R[A+4..=A+6] + `begin_call`.
    /// ipairs `inext` fast path at the top — skip begin_call
    ///     when R[A]=Native(ipairs_iter), R[A+1]=Table no-mt,
    ///     R[A+2]=Int.
    /// Batched out-ptr writeback — fill ctrl/key/val raws into
    ///     caller-provided buffers + return R[A+4]'s tag byte. Lets
    ///     emit skip 3 separate `luna_jit_stack_load` calls and 1
    ///     `luna_jit_stack_tag` call by reading the buffer via
    ///     cranelift `stack_load` IR instead. Returns -1 on deopt,
    ///     else R[A+4]'s tag byte | R[A+5]'s tag byte << 8 (the value's
    ///     tag only when `nvars >= 2`, 0 otherwise).
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `ctrl_out`/`key_out`/`val_out` are caller-stack buffers from Cranelift-emitted prologue; SAFETY documented below.
    pub fn jit_op_tforcall(
        &mut self,
        slot_offset: u32,
        nvars: i32,
        ctrl_out: *mut i64,
        key_out: *mut i64,
        val_out: *mut i64,
    ) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return -1;
        };
        let abs = f.base + slot_offset;
        let need = (abs + 7) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // ipairs fast path
        let took_fast_path = if let Value::Native(n) = self.stack[abs as usize]
            && std::ptr::fn_addr_eq(
                n.f,
                crate::vm::builtins::ipairs_iter as crate::runtime::value::NativeFn,
            )
            && let Value::Table(t) = self.stack[(abs + 1) as usize]
            && t.metatable().is_none()
            && let Value::Int(i) = self.stack[(abs + 2) as usize]
        {
            let next_i = i.wrapping_add(1);
            let v = t.get_int(next_i);
            if v.is_nil() {
                self.stack[(abs + 4) as usize] = Value::Nil;
            } else {
                self.stack[(abs + 4) as usize] = Value::Int(next_i);
                if (nvars as usize) >= 2 {
                    self.stack[(abs + 5) as usize] = v;
                }
                for j in 2..nvars as usize {
                    let slot = abs + 4 + j as u32;
                    if (slot as usize) < self.stack.len() {
                        self.stack[slot as usize] = Value::Nil;
                    }
                }
            }
            true
        } else {
            false
        };
        if !took_fast_path {
            // slow path: copy R[A..=A+2] → R[A+4..=A+6], then
            // route through begin_call. Lua-closure iters would push
            // a Lua frame mid-trace → deopt.
            self.stack[(abs + 4) as usize] = self.stack[abs as usize];
            self.stack[(abs + 5) as usize] = self.stack[(abs + 1) as usize];
            self.stack[(abs + 6) as usize] = self.stack[(abs + 2) as usize];
            // the interpreter raises the call's error itself; and a native
            // that `begin_call` hands to the interpreter loop (pcall, xpcall,
            // pairs, an async native) pushes frames or parks a future
            // instead of returning its results here
            let runs_to_completion = match self.stack[abs as usize] {
                Value::Native(nc) => nc.kind == NativeKind::Plain,
                _ => false,
            };
            if !runs_to_completion || self.begin_call(abs + 4, Some(2), nvars, false).is_err() {
                self.jit.counters.deopt += 1;
                return -1;
            }
        }
        // Batched writeback — fill the caller's buffers with the
        // raw bits of R[A+2] / R[A+4] / R[A+5] so the trace IR can
        // reload via cranelift `stack_load` instead of separate
        // `luna_jit_stack_load` helper calls.
        // SAFETY: every `RawVal` `unpack` returns has all 8 bytes initialised (`RawVal::NIL` for nil and booleans), so reading them as `zero` is defined
        let ctrl_raw = unsafe { self.stack[(abs + 2) as usize].unpack().1.zero };
        let (key_tag, key_rv) = self.stack[(abs + 4) as usize].unpack();
        // SAFETY: `key_rv` came from `unpack`, whose payload has all 8 bytes initialised
        let key_raw = unsafe { key_rv.zero };
        let (val_tag, val_raw) = if (nvars as usize) >= 2 {
            let (tag, rv) = self.stack[(abs + 5) as usize].unpack();
            // SAFETY: `rv` came from `unpack`, whose payload has all 8 bytes initialised
            (tag, unsafe { rv.zero })
        } else {
            (0, 0u64)
        };
        // SAFETY: the trace passes three 8-byte slots of its own stack frame, valid and unaliased for this call; the signature is safe, so nothing enforces that for other callers
        unsafe {
            ctrl_out.write(ctrl_raw as i64);
            key_out.write(key_raw as i64);
            val_out.write(val_raw as i64);
        }
        i64::from(key_tag) | i64::from(val_tag) << 8
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

    /// Push a Lua frame onto the call stack with
    /// JIT-known metadata. Used by `luna_jit_trace_materialize_frames`
    /// at trace side-exits to recreate the inlined call activations
    /// the lowerer compiled past. The contract (enforced by the
    /// lowerer's pre-emit pass): `cl.proto` is non-vararg,
    /// `nresults` is the caller's expected count (today always 1
    /// because the lowerer bails Op::Call C != 2), and the caller
    /// has already called `jit_ensure_stack` to cover
    /// `[0..base + cl.proto.max_stack)`.
    #[doc(hidden)]
    pub fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
    ) {
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc,
                // Lua call ABI: callee R[0] sits at caller R[A+1], so
                // callee.base = caller.base + A + 1; func_slot is
                // caller.base + A = callee.base - 1.
                func_slot: base - 1,
                n_varargs: 0,
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
}
