//! What compiled code's helpers do to the frames and registers of the
//! Vm they run in.

use crate::runtime::Frame;
use crate::runtime::LuaClosure;
use crate::runtime::Table;
use crate::runtime::Value;
use crate::runtime::heap::Gc;
use crate::vm::LuaError;
use crate::vm::exec::JitLimits;

/// The internals of `Vm` luna's other crates use.
pub trait VmJitFrames {
    /// Record the frame of the compiled level at `base` (level 0's when
    /// `None`), which the interpreter goes on with at `pc`, its registers
    /// `vals`.
    ///
    /// # Safety
    /// `base` and `pc` are a level compiled code is running: `pc` an instruction of that level's function, `vals` its registers.
    unsafe fn jit_handoff_frame(&mut self, base: Option<u32>, pc: u32, vals: Vec<Value>);
    /// The levels compiled code may enter from the level at `base`
    /// before it calls `Vm::jit_self_level` again. One bound on the base
    /// covers both what can happen: a level whose arguments' top plus its
    /// frame reaches the stack's end tests the stack; the level that grows
    /// 5.1's array or takes a new `CallInfo` (and in 5.2 any level while a
    /// collection step could fall due) is a number of levels deeper, each
    /// at least the smallest self call's step above the one before.
    fn jit_limits(&self, cl: Gc<LuaClosure>, base: u32) -> JitLimits;
    /// Level `depth` (1 for the first self call) of compiled `cl`, its
    /// frame at `base`, entered as PUC enters a call (with `tail`, the
    /// callee of a tail call PUC places there before it moves it down):
    /// the stack its frame needs, its `CallInfo`, 5.2's collection check;
    /// then the limits for the levels after it. `None` when the check would
    /// find a step due: the interpreter makes the call instead, the level
    /// not counted here.
    fn jit_self_level(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        depth: u32,
        tail: bool,
    ) -> Result<Option<JitLimits>, LuaError>;
    /// Last live Lua frame (the trace head's frame at
    /// dispatch time). The frame-materialization helper reads `.base`
    /// to compute offsets for each inlined frame's window.
    fn jit_last_lua_frame(&self) -> Option<Frame>;
    /// Ensure the value stack covers indices
    /// `[0..need)`. Extends with Nil if shorter. Called by the
    /// frame-materialization helper before pushing an inlined frame
    /// whose register window may exceed the current stack length.
    /// `false` when the stack cannot grow: the caller parks a deopt.
    fn jit_ensure_stack(&mut self, need: usize) -> bool;
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
    fn jit_op_close(&mut self, start_offset: u32) -> i64;
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
    fn jit_spill_stack(&mut self, slot_offset: u32, v: Value);
    /// `vm.stack[base + slot_offset]` of the trace's head frame, or
    /// `None` when there is no Lua frame or the slot is past the
    /// stack. The trace JIT's Op::Concat body emit refreshes the
    /// payload of a slot whose `RegKind` is `Unset` (no compile-time
    /// tag info; commonly `Str` slots which the trace doesn't model)
    /// through it, keeping the tag the interpreter left there.
    fn jit_stack_slot_mut(&mut self, slot_offset: u32) -> Option<&mut Value>;
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
    fn jit_op_concat(&mut self, slot_offset: u32, n: i32) -> i64;
    /// Load the raw `i64` payload of
    /// `vm.stack[base + slot_offset]` for the active trace's head
    /// Lua frame. Used to reload trace IR `Variable`s after a
    /// helper has written to `vm.stack` directly (e.g. TForCall's
    /// iter results land at `R[A+4..A+4+nvars]`).
    fn jit_stack_load(&mut self, slot_offset: u32) -> i64;
    /// Read the tag byte of
    /// `vm.stack[base + slot_offset]`. Used by `Op::TForLoop` emit
    /// to dispatch on the iterator's return-key tag at runtime
    /// (`raw::NIL` → loop end exit, `raw::INT` → continue, other →
    /// deopt).
    fn jit_stack_tag(&mut self, slot_offset: u32) -> u8;
    /// Set the stack top to register `rel` of the running trace's head
    /// frame: where the interpreter would leave it after a call that
    /// returned every value, or a vararg expansion, for an op that reads
    /// it (a call or return of a variable count) and an exit before that
    /// op.
    ///
    /// # Safety
    /// `rel` lies within the registers of the trace head frame that hold live values: the values below the new top are read as live (passed to calls, returned, marked by the collector).
    unsafe fn jit_set_top(&mut self, rel: u32);
    /// Push a Lua frame onto the call stack with JIT-known metadata, for
    /// `luna_jit_trace_materialize_frames` at a trace exit inside a
    /// function the trace inlined. `nresults` is the caller's wanted count
    /// (-1 for all); a vararg `cl` has `n_varargs` extra arguments just
    /// below `base`, the function one below them, as `push_frame` leaves
    /// them. The caller has already called `jit_ensure_stack` to cover
    /// `[0..base + cl.proto.max_stack)`.
    /// `false`, pushing nothing, when the frame stack cannot grow.
    ///
    /// # Safety
    /// `cl` is the function compiled code inlined at this point, `pc` an instruction of its prototype, `base` and `n_varargs` its frame as compiled code laid it out, and the stack covers `base + max_stack` (`VmInternals::jit_ensure_stack`).
    unsafe fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
        n_varargs: u32,
    ) -> bool;
    /// Upvalue `idx` of `cl`, for compiled code running a function a trace
    /// inlined, whose head frame starts at `head_base`: `None` when the
    /// upvalue is open at a slot of the running thread at or above
    /// `head_base`, a register the trace may hold only in its own
    /// registers while the stack has an older value.
    fn jit_upval_below(
        &self,
        cl: Gc<LuaClosure>,
        idx: u32,
        head_base: u32,
    ) -> Option<crate::runtime::Value>;
    /// `t[key]` through table-valued `__index` links (up to four, as the
    /// interpreter's fast path follows), for compiled code: `None` when a
    /// link is a function or the chain goes on, which the interpreter has
    /// to run.
    fn jit_index_str_tables(
        &self,
        t: Gc<Table>,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<crate::runtime::Value>;
    /// Trace JIT helper for a generic `TForCall A 0 C` of any layout:
    /// `nvars` packs C with the layout's registers (`ForLayout::pack_call`);
    /// below, A+4 stands for the first variable and A+2 for the control
    /// (5.4's layout).
    ///
    /// Base path: copy `R[A]`, `R[A+1]`, `R[A+2]` to `R[A+4..=A+6]` and
    /// `begin_call`. ipairs `inext` fast path at the top: skip begin_call
    /// when `R[A]` is `Native(ipairs_iter)`, `R[A+1]` a table without a
    /// metatable, `R[A+2]` an integer.
    /// Batched out-ptr writeback — fill ctrl/key/val raws into
    ///     caller-provided buffers + return `R[A+4]`'s tag byte. Lets
    ///     emit skip 3 separate `luna_jit_stack_load` calls and 1
    ///     `luna_jit_stack_tag` call by reading the buffer via
    ///     cranelift `stack_load` IR instead. Returns -1 on deopt,
    ///     else `R[A+4]`'s tag byte | `R[A+5]`'s tag byte << 8 (the value's
    ///     tag only when `nvars >= 2`, 0 otherwise).
    fn jit_op_tforcall(
        &mut self,
        slot_offset: u32,
        nvars: i32,
        ctrl_out: &mut i64,
        key_out: &mut i64,
        val_out: &mut i64,
    ) -> i64;
}

impl VmJitFrames for crate::vm::Vm {
    #[inline]
    unsafe fn jit_handoff_frame(&mut self, base: Option<u32>, pc: u32, vals: Vec<Value>) {
        crate::vm::Vm::jit_handoff_frame(self, base, pc, vals)
    }
    #[inline]
    fn jit_limits(&self, cl: Gc<LuaClosure>, base: u32) -> JitLimits {
        crate::vm::Vm::jit_limits(self, cl, base)
    }
    #[inline]
    fn jit_self_level(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        depth: u32,
        tail: bool,
    ) -> Result<Option<JitLimits>, LuaError> {
        crate::vm::Vm::jit_self_level(self, cl, base, depth, tail)
    }
    #[inline]
    fn jit_last_lua_frame(&self) -> Option<Frame> {
        crate::vm::Vm::jit_last_lua_frame(self)
    }
    #[inline]
    fn jit_ensure_stack(&mut self, need: usize) -> bool {
        crate::vm::Vm::jit_ensure_stack(self, need)
    }
    #[inline]
    fn jit_op_close(&mut self, start_offset: u32) -> i64 {
        crate::vm::Vm::jit_op_close(self, start_offset)
    }
    #[inline]
    fn jit_spill_stack(&mut self, slot_offset: u32, v: Value) {
        crate::vm::Vm::jit_spill_stack(self, slot_offset, v)
    }
    #[inline]
    fn jit_stack_slot_mut(&mut self, slot_offset: u32) -> Option<&mut Value> {
        crate::vm::Vm::jit_stack_slot_mut(self, slot_offset)
    }
    #[inline]
    fn jit_op_concat(&mut self, slot_offset: u32, n: i32) -> i64 {
        crate::vm::Vm::jit_op_concat(self, slot_offset, n)
    }
    #[inline]
    fn jit_stack_load(&mut self, slot_offset: u32) -> i64 {
        crate::vm::Vm::jit_stack_load(self, slot_offset)
    }
    #[inline]
    fn jit_stack_tag(&mut self, slot_offset: u32) -> u8 {
        crate::vm::Vm::jit_stack_tag(self, slot_offset)
    }
    #[inline]
    unsafe fn jit_set_top(&mut self, rel: u32) {
        crate::vm::Vm::jit_set_top(self, rel)
    }
    #[inline]
    unsafe fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
        n_varargs: u32,
    ) -> bool {
        crate::vm::Vm::jit_push_inlined_frame(self, cl, base, pc, nresults, n_varargs)
    }
    #[inline]
    fn jit_upval_below(
        &self,
        cl: Gc<LuaClosure>,
        idx: u32,
        head_base: u32,
    ) -> Option<crate::runtime::Value> {
        crate::vm::Vm::jit_upval_below(self, cl, idx, head_base)
    }
    #[inline]
    fn jit_index_str_tables(
        &self,
        t: Gc<Table>,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<crate::runtime::Value> {
        crate::vm::Vm::jit_index_str_tables(self, t, key)
    }
    #[inline]
    fn jit_op_tforcall(
        &mut self,
        slot_offset: u32,
        nvars: i32,
        ctrl_out: &mut i64,
        key_out: &mut i64,
        val_out: &mut i64,
    ) -> i64 {
        crate::vm::Vm::jit_op_tforcall(self, slot_offset, nvars, ctrl_out, key_out, val_out)
    }
}
