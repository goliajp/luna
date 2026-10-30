//! Runtime entry points the JIT-compiled code calls back into.

use super::*;

mod recording_check;
#[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
mod recording_live;
mod sunk;
pub(crate) use sunk::TraceGhosts;
mod upvals;

impl Vm {
    /// Last live Lua frame (the trace head's frame at
    /// dispatch time). The frame-materialization helper reads `.base`
    /// to compute offsets for each inlined frame's window.
    pub(crate) fn jit_last_lua_frame(&self) -> Option<Frame> {
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
    pub(in crate::vm) fn inspect_frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// Ensure the value stack covers indices
    /// `[0..need)`. Extends with Nil if shorter. Called by the
    /// frame-materialization helper before pushing an inlined frame
    /// whose register window may exceed the current stack length.
    /// `false` when the stack cannot grow: the caller parks a deopt.
    pub(crate) fn jit_ensure_stack(&mut self, need: usize) -> bool {
        self.stack.len() >= need || self.stack.resize(need, Value::Nil).is_ok()
    }

    /// Park a deopt for an allocation compiled code could not make: the
    /// dispatcher drops what the compiled code computed and the
    /// interpreter runs the code again, allocating through its own path
    /// (which may collect in an emergency, as compiled code may not: it
    /// can hold new objects only in machine registers). The parked value
    /// is never raised, so it allocates nothing.
    pub(crate) fn jit_park_oom(&mut self) {
        if self.jit.pending_err.is_none() {
            self.jit.pending_err = Some(LuaError(Value::Nil));
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
    pub(crate) fn jit_op_close(&mut self, start_offset: u32) -> i64 {
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
    pub(crate) fn jit_spill_stack(&mut self, slot_offset: u32, v: Value) {
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err =
                Some(self.rt_err("JIT spill: no Lua frame on jit_last_lua_frame()"));
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if !self.jit_ensure_stack(idx + 1) {
            self.jit_park_oom();
            return;
        }
        self.stack[idx] = v;
    }

    /// [`crate::__private::VmJitFrames::jit_concat_regs`]: the interpreter's
    /// concat at register `a` of the trace's head frame, its operands
    /// stored from `ops` first. A metamethod frame `concat_run` pushed is
    /// unwound, and the interpreter redoes the op.
    pub(crate) fn jit_concat_regs(&mut self, a: u32, ops: &mut [[i64; 2]]) -> i64 {
        if self.jit.pending_err.is_some() {
            return 0;
        }
        let Some(CallFrame::Lua(f)) = self.frames.last() else {
            return 0;
        };
        let at = f.base + a;
        let (lo, n) = (at as usize, ops.len());
        // the frame's registers lie on the stack
        let Some(regs) = self.stack.get_mut(lo..lo + n) else {
            return 0;
        };
        for (slot, op) in regs.iter_mut().zip(ops.iter()) {
            let tag = u8::try_from(op[0]).unwrap_or_else(|_| slot.unpack().0);
            let raw = crate::runtime::value::RawVal { zero: op[1] as u64 };
            // SAFETY: the trace passes each register's own tag and payload, or
            // a payload of the type the slot holds now with no tag
            *slot = unsafe { Value::pack(tag, raw) };
        }
        self.top = at + n as u32;
        let frames = self.frames.len();
        // the check for a collection is the interpreter's, at the exit the
        // trace takes after the op when a step is due
        let result = self.concat_run(at, at);
        if result.is_err() || self.frames.len() > frames {
            while self.frames.len() > frames {
                self.pop_frame();
            }
            self.jit.counters.deopt += 1;
            return 0;
        }
        let payload = |v: Value| {
            // SAFETY: every `RawVal` `unpack` returns has all 8 bytes initialised
            unsafe { v.unpack().1.zero as i64 }
        };
        for (op, &v) in ops[1..].iter_mut().zip(&self.stack[lo + 1..lo + n]) {
            op[1] = payload(v);
        }
        payload(self.stack[lo])
    }

    /// Read the tag byte of
    /// `vm.stack[base + slot_offset]`. Used by `Op::TForLoop` emit
    /// to dispatch on the iterator's return-key tag at runtime
    /// (`raw::NIL` → loop end exit, `raw::INT` → continue, other →
    /// deopt).
    pub(crate) fn jit_stack_tag(&mut self, slot_offset: u32) -> u8 {
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
    pub(crate) fn jit_set_top(&mut self, rel: u32) {
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
    /// `false`, pushing nothing, when the frame stack cannot grow.
    pub(crate) fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
        n_varargs: u32,
    ) -> bool {
        // room first, so the push below never has to allocate
        if self.frames.reserve(1).is_err() {
            return false;
        }
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
        true
    }
}
