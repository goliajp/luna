//! What a JIT entry saves of the thread's current Vm and closure, to put
//! back when it leaves.

/// Capture of the previous `(JIT_VM, JIT_CL)` slot contents at the
/// moment `JitVmGuard` takes ownership, plus a function pointer
/// that writes them back. The function pointer lets luna-core hold
/// the drop semantics without referencing Cranelift's TLS storage
/// (the cells themselves live in `luna_jit::jit_backend` because
/// that's where the `thread_local!` block is, but the cells hold
/// luna-core types — `*mut Vm` and `*const LuaClosure`).
///
/// Fields are `pub` rather than `pub(crate)` because luna-jit's
/// `enter_jit` needs to construct an instance, and luna-jit is a
/// separate crate; it reaches the type through `luna_core::__private`,
/// outside the embedder surface.
pub struct JitVmRebindRestore {
    /// the Vm the thread ran before the entry
    pub prev_vm: *mut crate::vm::Vm,
    /// the closure it ran
    pub prev_cl: *const crate::runtime::LuaClosure,
    /// luna-jit-side function that writes `(prev_vm, prev_cl)` back
    /// into the TLS cells. Set by `enter_jit`. It only stores the two
    /// pointers; the helpers that read them are the ones with a
    /// contract.
    pub restore_fn: fn(prev_vm: *mut crate::vm::Vm, prev_cl: *const crate::runtime::LuaClosure),
}
