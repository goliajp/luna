//! What the trace compiler records on its thread for diagnosis: the phase
//! and op it last reached, and how many traces it compiled.

// last-checkpoint instrumentation for trace
// compile failure diagnosis. `try_compile_trace_with_options`
// updates the thread-local at each major phase; if the function
// returns `None`, the most recent checkpoint set tells the
// caller WHICH phase bailed. Vm reads + accumulates this on
// every compile-failed return.
thread_local! {
    pub(crate) static LAST_COMPILE_CHECKPOINT: std::cell::Cell<&'static str> =
        const { std::cell::Cell::new("not-entered") };
    pub(crate) static LAST_OP_ID: std::cell::Cell<u8> =
        const { std::cell::Cell::new(255) };
    /// The index in the recording of the op `LAST_OP_ID` names
    /// (`usize::MAX`: none yet).
    pub(crate) static LAST_OP_IDX: std::cell::Cell<usize> =
        const { std::cell::Cell::new(usize::MAX) };
    // counter bumped exactly once per
    // `lower_trace_into_named` invocation that successfully declares
    // the depth-relative `base_var` scaffold. Used by the regression
    // test (`base_var_scaffold.rs`) to assert the
    // scaffold's declaration ran end-to-end.
    //
    // Probe-only: dispatched + close-cause counters cover production
    // behaviour; this cell exists solely so the test can pin "scaffold
    // ran" without scraping Cranelift IR text. The bump happens AFTER
    // `declare_var` + `def_var(iconst(0))` so a panic earlier in the
    // entry block leaves the counter at its prior value.
    pub(crate) static BASE_VAR_SCAFFOLD_DECLARED: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
    pub(in crate::jit_backend::trace) static TRACE_CODEGEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static TRACE_CODE_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(in crate::jit_backend::trace) fn checkpoint(s: &'static str) {
    LAST_COMPILE_CHECKPOINT.with(|c| c.set(s));
}

pub(in crate::jit_backend::trace) fn set_last_op_id(id: u8) {
    LAST_OP_ID.with(|c| c.set(id));
}

/// The lowerer is at op `idx` of the recording, of opcode `id`.
pub(in crate::jit_backend::trace) fn set_last_op(idx: usize, id: u8) {
    set_last_op_id(id);
    LAST_OP_IDX.with(|c| c.set(idx));
}

/// The index in the recording of the op the lowerer last checked or
/// emitted on this thread, if it got to one since `plan_trace` began.
#[doc(hidden)]
pub fn last_op_index() -> Option<usize> {
    Some(LAST_OP_IDX.with(|c| c.get())).filter(|&i| i != usize::MAX)
}

/// Name of the lowerer checkpoint most recently reached on this thread.
/// Diagnostic-only — used to bucket trace-compile failures by phase
/// (`pre-lower`, `lower-loop`, `finalize`, …).
pub fn last_compile_checkpoint() -> &'static str {
    LAST_COMPILE_CHECKPOINT.with(|c| c.get())
}

/// Opcode id (luna `Op` discriminant) of the last bytecode op the
/// lowerer touched on this thread. Diagnostic-only.
pub fn last_op_id() -> u8 {
    LAST_OP_ID.with(|c| c.get())
}

/// count of successful `base_var` scaffold
/// declarations on this thread. Bumped exactly once per
/// `lower_trace_into_named` invocation that reaches the post-entry
/// emit point and runs `declare_var` + `def_var(iconst(0))` for the
/// depth-relative base address handle. No op-arm reads it: the
/// Variable is in-scope for the entire lowerer body but
/// `use_var(base_var)` doesn't happen.
///
/// Read by `base_var_scaffold.rs`; production paths
/// (dispatcher / close handler / vm) never read this.
pub fn base_var_scaffold_declared_count() -> u64 {
    BASE_VAR_SCAFFOLD_DECLARED.with(|c| c.get())
}

/// Traces this thread has compiled into machine code (either tier) for
/// a Vm or via [`try_compile_trace_with_options`](super::try_compile_trace_with_options).
/// Diagnostic-only: it tells a trace cached with code from one a Vm
/// cached without, because nothing could enter it.
#[doc(hidden)]
pub fn trace_codegen_count() -> u64 {
    TRACE_CODEGEN.with(|c| c.get())
}

/// Bytes of machine code this thread placed for traces (either tier, and
/// the copies of code made for another Vm). Diagnostic-only: what the
/// code a run compiled takes.
#[doc(hidden)]
pub fn trace_code_bytes() -> u64 {
    TRACE_CODE_BYTES.with(|c| c.get())
}

pub(in crate::jit_backend::trace) fn note_code_bytes(n: usize) {
    TRACE_CODE_BYTES.with(|c| c.set(c.get() + n as u64));
}

/// reset the scaffold-declared counter so
/// a regression test can assert "the next compile bumped it by 1"
/// without depending on prior tests in the same thread. Test-only;
/// production paths never call this.
pub fn reset_base_var_scaffold_declared_count() {
    BASE_VAR_SCAFFOLD_DECLARED.with(|c| c.set(0));
}
