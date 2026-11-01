//! What compiling a trace came to, and what an inlined call held.

/// Result of attempting to lower a closed
/// [`TraceRecord`](crate::__private::TraceRecord) to native
/// code. Most failure cases are recoverable — the recorder bumps the
/// head PC's failure count and refuses to re-record until the
/// threshold rolls over again.
#[derive(Debug)]
pub enum CompileOutcome {
    /// Trace compiled; the cached entry is ready for dispatch.
    Compiled,
    /// Some op in the trace falls outside the lowerer's whitelist (e.g. a
    /// metamethod-bearing operand, or a yet-unsupported opcode).
    /// The record is dropped; the head PC remembers the rejection.
    UnsupportedOp,
    /// Cranelift signaled an error during code emission. Should be
    /// rare in practice — usually a programmer error in the lowerer.
    BackendError,
}

/// [`CompiledTraceParts::inline_kinds`](crate::__private::CompiledTraceParts::inline_kinds):
/// a call into a vararg function.
pub const INLINE_VARARG_CALLEE: u8 = 1;
/// A call that wants more than one value, or all of them.
pub const INLINE_MULTI_RESULTS: u8 = 1 << 1;
/// A call that passes a variable number of arguments.
pub const INLINE_VAR_ARGS: u8 = 1 << 2;
/// A closure made in a frame of an inlined function.
pub const INLINE_CLOSURE: u8 = 1 << 3;
