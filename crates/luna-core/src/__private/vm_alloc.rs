//! What compiled code's helpers allocate and count, as the interpreter
//! would at the same op.

use crate::runtime::LuaClosure;
use crate::runtime::Table;
use crate::runtime::heap::Gc;
use crate::runtime::mem::LVec;
use crate::vm::Vm;

/// The internals of `Vm` luna's other crates use.
pub trait VmJitAlloc {
    /// Compiled code just allocated (or 5.2 entered a call): whether a
    /// collection step is due, noted with the op whose check it is
    /// (`op_pc`, as `Vm::checkpoint_after_jit` takes it) for the exit it
    /// then takes to have the interpreter check.
    fn jit_gc_due(&mut self, op_pc: u32) -> bool;
    /// The registers a step run in place in the running trace writes back
    /// at its exit `exit_ret` (0: the step cannot run there).
    fn jit_step_window(&self, exit_ret: u64) -> usize;
    /// Compiled code reached the due point after the op whose check
    /// `op_pc` names; `regs` the trace frame's registers as of its exit
    /// `exit_ret` when [`VmJitAlloc::jit_step_window`] allowed them: 0 to
    /// go on, otherwise leave through that exit.
    fn jit_gc_step(&mut self, op_pc: u32, exit_ret: u64, regs: Option<&[i64]>) -> i64;
    /// The count and, [`crate::jit::JIT_GC_LIMIT_OFFSET`] bytes after it,
    /// the point a collection step falls due at: compiled code calls
    /// [`VmJitAlloc::jit_gc_due`] only once the count has reached it.
    fn jit_due_words(&self) -> *const usize;
    /// OP_CLOSURE for compiled code, in a frame of `cl` whose registers
    /// start at `base`: `None` when an allocation fails (compiled code may
    /// hold new objects only in registers, so no emergency collection
    /// runs; the interpreter then makes the closure).
    ///
    /// # Safety
    /// `cl` is the function of the running frame whose registers start at `base`, and `idx` one of its prototypes: the new closure captures registers of that frame.
    unsafe fn jit_closure(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        base: u32,
    ) -> Option<Gc<LuaClosure>>;
    /// Whether compiled code's `NewTable` of operands `b`, `c`, `k` would
    /// find a collection step due when it checks after making the table
    /// (PUC's size of it, as the operands give it): then the interpreter
    /// runs it. A table the operands cannot give is the interpreter's too.
    fn jit_table_due(&self, b: u32, c: u32, k: bool) -> bool;
    /// Park a deopt for an allocation compiled code could not make: the
    /// dispatcher drops what the compiled code computed and the
    /// interpreter runs the code again, allocating through its own path
    /// (which may collect in an emergency, as compiled code may not: it
    /// can hold new objects only in machine registers). The parked value
    /// is never raised, so it allocates nothing.
    fn jit_park_oom(&mut self);
    /// A trace's sunk site `site` makes its table this round (a `NewTable`
    /// sized by `b`, `c`, `k`): `true` when a collection step falls due
    /// with it, the trace then leaving with the table made for real;
    /// otherwise it is counted and recorded.
    fn jit_sunk_table(&mut self, site: usize, b: u32, c: u32, k: bool) -> bool;
    /// Sunk site `site`'s table grows from `old_h` nodes to `new_h` (a
    /// field it had no room for; the site has no array part).
    fn jit_sunk_grow(&mut self, site: usize, old_h: usize, new_h: usize);
    /// A trace leaves with sunk site `site`'s table, which `make` makes
    /// for real now (`None` when it cannot): when PUC's was counted this
    /// round, it is made without counting and stands for that one.
    fn jit_sunk_make(
        &mut self,
        site: usize,
        make: impl FnOnce(&mut Vm) -> Option<Gc<Table>>,
    ) -> Option<Gc<Table>>;
    /// Pop a reusable buffer from the JIT accumulator buffer pool, or
    /// make an empty one when the pool is empty. The trace keeps it (as
    /// the boxed pointer the helper leaks) in a stack slot through the
    /// loop and appends each piece to it.
    fn jit_str_buf_acquire(&mut self) -> Box<LVec<u8>>;
    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend buffer.
    fn jit_str_buf_release(&mut self, buf: Box<LVec<u8>>);
    /// Append a piece's bytes to an accumulator buffer; `false`, the
    /// buffer as it was, when it cannot grow.
    fn jit_str_buf_extend(&mut self, buf: &mut LVec<u8>, piece: Gc<crate::runtime::LuaStr>)
    -> bool;
    /// The `CONCAT` at `op_pc` of a trace's accumulated concatenation:
    /// `piece` appended to the buffer, and the string PUC makes of the two
    /// counted (a short one made for real, so that it is found as PUC's
    /// is; a long one recorded, which the buffer's string stands for once
    /// made). -1 when the buffer cannot grow; 1 when a collection step
    /// falls due, the trace then leaving after the op.
    fn jit_str_buf_concat(
        &mut self,
        buf: &mut LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
        op_pc: u32,
    ) -> i64;
    /// Drain the accumulator buffer into a fresh string, returning its raw
    /// pointer bits for the trace to write into the accumulator slot. The
    /// buffer is left empty, its capacity kept, ready for release.
    ///
    /// Returns 0 when the string is past the hard cap or cannot be
    /// allocated; a deopt is parked then, so the dispatcher drops the
    /// trace's registers (the 0 among them) and the interpreter runs the
    /// loop again.
    fn jit_str_buf_intern(&mut self, buf: &mut LVec<u8>) -> i64;
}

impl VmJitAlloc for crate::vm::Vm {
    #[inline]
    fn jit_gc_due(&mut self, op_pc: u32) -> bool {
        crate::vm::Vm::jit_gc_due(self, op_pc)
    }
    #[inline]
    fn jit_step_window(&self, exit_ret: u64) -> usize {
        crate::vm::Vm::jit_step_window(self, exit_ret)
    }
    #[inline]
    fn jit_gc_step(&mut self, op_pc: u32, exit_ret: u64, regs: Option<&[i64]>) -> i64 {
        crate::vm::Vm::jit_gc_step(self, op_pc, exit_ret, regs)
    }
    #[inline]
    fn jit_due_words(&self) -> *const usize {
        crate::vm::Vm::jit_due_words(self)
    }
    #[inline]
    unsafe fn jit_closure(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        base: u32,
    ) -> Option<Gc<LuaClosure>> {
        crate::vm::Vm::jit_closure(self, cl, idx, base)
    }
    #[inline]
    fn jit_table_due(&self, b: u32, c: u32, k: bool) -> bool {
        crate::vm::Vm::jit_table_due(self, b, c, k)
    }
    #[inline]
    fn jit_park_oom(&mut self) {
        crate::vm::Vm::jit_park_oom(self)
    }
    #[inline]
    fn jit_sunk_table(&mut self, site: usize, b: u32, c: u32, k: bool) -> bool {
        crate::vm::Vm::jit_sunk_table(self, site, b, c, k)
    }
    #[inline]
    fn jit_sunk_grow(&mut self, site: usize, old_h: usize, new_h: usize) {
        crate::vm::Vm::jit_sunk_grow(self, site, old_h, new_h)
    }
    #[inline]
    fn jit_sunk_make(
        &mut self,
        site: usize,
        make: impl FnOnce(&mut Vm) -> Option<Gc<Table>>,
    ) -> Option<Gc<Table>> {
        crate::vm::Vm::jit_sunk_make(self, site, make)
    }
    #[inline]
    fn jit_str_buf_acquire(&mut self) -> Box<LVec<u8>> {
        crate::vm::Vm::jit_str_buf_acquire(self)
    }
    #[inline]
    fn jit_str_buf_release(&mut self, buf: Box<LVec<u8>>) {
        crate::vm::Vm::jit_str_buf_release(self, buf)
    }
    #[inline]
    fn jit_str_buf_extend(
        &mut self,
        buf: &mut LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
    ) -> bool {
        crate::vm::Vm::jit_str_buf_extend(self, buf, piece)
    }
    #[inline]
    fn jit_str_buf_concat(
        &mut self,
        buf: &mut LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
        op_pc: u32,
    ) -> i64 {
        crate::vm::Vm::jit_str_buf_concat(self, buf, piece, op_pc)
    }
    #[inline]
    fn jit_str_buf_intern(&mut self, buf: &mut LVec<u8>) -> i64 {
        crate::vm::Vm::jit_str_buf_intern(self, buf)
    }
}
