//! JIT-backend trait boundary (luna-core side).
//!
//! Defines the [`IntChunkCompiler`] + [`TraceCompiler`] traits that
//! the interpreter dispatcher in [`crate::vm::exec`] routes through
//! to invoke the JIT codegen. Two implementations exist:
//!
//! - [`NullJitBackend`] (in this file) — does nothing; the default
//!   for [`crate::vm::Vm::new_minimal`] in luna-core.
//! - `CraneliftBackend` (in the `luna` crate, `luna::jit_backend`) —
//!   delegates to Cranelift. Installed by `luna::install_default_jit`
//!   and `luna::Vm::new_minimal_with_jit`.
//!
//! v1.1 A1 Session C moved this file from `src/jit/abi.rs` (single
//! crate) to `crates/luna-core/src/jit/abi.rs` (workspace). The trait
//! surface itself is unchanged from Session A.
//!
//! RFC: `.dev/rfcs/v1.1-rfc-crate-split.md` §D1 + Migration step 6.

use crate::jit::JitVmGuard;
use crate::jit::trace_types::{CompileOptions, CompiledTrace, TraceRecord};
use crate::runtime::Gc;
use crate::runtime::LuaClosure;
use crate::runtime::function::Proto;

/// Native entry-point produced by `try_compile_int_chunk` for a chunk
/// with zero params. Returns the chunk's `Return1` value as i64;
/// `Return0` chunks return 0 (the caller knows by inspecting
/// `JitHandle::returns_one`).
// SAFETY: offset is hand-computed against the `repr(C)` layout of the target struct in this same module; the Cranelift lowerer relies on it staying in sync.
pub type IntChunkFn = unsafe extern "C" fn() -> i64;

/// One-arg JIT entry signature. See [`IntChunkFn`] for the zero-arg
/// shape and [`MAX_JIT_ARITY`] for the cap.
// SAFETY: offset is hand-computed against the `repr(C)` layout of the target struct in this same module; the Cranelift lowerer relies on it staying in sync.
pub type IntFn1 = unsafe extern "C" fn(i64) -> i64;
/// Two-arg JIT entry signature.
// SAFETY: offset is hand-computed against the `repr(C)` layout of the target struct in this same module; the Cranelift lowerer relies on it staying in sync.
pub type IntFn2 = unsafe extern "C" fn(i64, i64) -> i64;
/// Three-arg JIT entry signature.
// SAFETY: offset is hand-computed against the `repr(C)` layout of the target struct in this same module; the Cranelift lowerer relies on it staying in sync.
pub type IntFn3 = unsafe extern "C" fn(i64, i64, i64) -> i64;
/// Four-arg JIT entry signature.
// SAFETY: offset is hand-computed against the `repr(C)` layout of the target struct in this same module; the Cranelift lowerer relies on it staying in sync.
pub type IntFn4 = unsafe extern "C" fn(i64, i64, i64, i64) -> i64;

/// Max arity the int-chunk compiler accepts before bailing back to
/// the interpreter. Tuned against the 5-arg-and-down distribution in
/// the official PUC test suites — extending past 4 buys very little
/// hit-rate and complicates the dispatch table in [`crate::vm::exec`].
pub const MAX_JIT_ARITY: u8 = 4;

/// Outcome of a closure-compilation attempt. Kept ABI-compatible with
/// the legacy `crate::jit::cache_lookup_or_compile` return tuple so the
/// `Vm::populate_jit_cache` call site doesn't have to re-shape its
/// destructure.
#[derive(Clone, Copy, Debug)]
pub enum CompileResult {
    /// The proto was lowered (or served from cache); the fields mirror
    /// the legacy 7-tuple returned by `cache_lookup_or_compile`.
    Compiled {
        entry: *const u8,
        num_args: u8,
        returns_one: bool,
        arg_float_mask: u8,
        arg_table_mask: u8,
        ret_is_float: bool,
        ret_is_table: bool,
    },
    /// The proto fell outside the whitelist or its compile pass bailed.
    /// The interpreter handles it unchanged.
    Skipped,
}

/// Closure-compilation backend. The interpreter dispatcher (and the
/// `Op::Call` JIT fast path inside `vm/exec.rs`) calls
/// `chunk_compiler.try_compile(proto, pre53, float_only)` once per
/// Proto on the cold path; subsequent hits read the result back from
/// `Proto.jit: Cell<JitProtoState>` directly, so the vtable cost is
/// bounded by Proto count, not by call count.
///
/// `enter` is the per-JIT-entry RAII guard that pins the active `Vm`
/// pointer (and optional `LuaClosure`) into the thread-locals the JIT
/// helpers read. Taking a raw `*mut Vm` keeps the trait object-safe
/// and lets the dispatcher pass `self as *mut Vm` without holding a
/// mutable borrow on `self` while reading `self.chunk_compiler`.
pub trait IntChunkCompiler {
    fn try_compile(&self, proto: Gc<Proto>, pre53: bool, float_only: bool) -> CompileResult;

    /// Install the active `Vm` + closure pointer into the JIT
    /// helpers' thread-local slots; the returned guard restores
    /// state on drop. For [`NullJitBackend`] this is a no-op
    /// (helpers never fire because nothing compiled).
    ///
    /// SAFETY: the caller must ensure `vm` is a live, exclusively
    /// borrowed `Vm` for the duration of the returned guard.
    fn enter(&self, vm: *mut crate::vm::Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard;
}

/// Trace-JIT backend. Receives a closed [`TraceRecord`] from the
/// interpreter's recorder; returns `Some(CompiledTrace)` on success or
/// `None` when the lowerer bailed. `last_compile_checkpoint` exposes
/// the lowerer's per-thread last-phase marker used by
/// `Vm.trace_compile_failed_reasons`.
pub trait TraceCompiler {
    fn try_compile_trace(
        &self,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace>;

    fn last_compile_checkpoint(&self) -> &'static str;
}

/// No-op backend installed by [`crate::vm::Vm::new_minimal`] in
/// luna-core. Both calls report "nothing compiled" so the interp
/// dispatcher always takes the standard path. Embedders who want a
/// real JIT either depend on the `luna` crate (whose
/// `Vm::new_minimal_with_jit` swaps in `CraneliftBackend`) or write
/// their own `IntChunkCompiler` / `TraceCompiler` implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullJitBackend;

impl IntChunkCompiler for NullJitBackend {
    fn try_compile(&self, _: Gc<Proto>, _: bool, _: bool) -> CompileResult {
        CompileResult::Skipped
    }

    fn enter(&self, _: *mut crate::vm::Vm, _: Option<Gc<LuaClosure>>) -> JitVmGuard {
        // The TLS slots stay whatever they were — no JIT mcode will
        // execute under a NullJitBackend (try_compile returned
        // Skipped, so no Proto reaches JitProtoState::Compiled, and
        // try_compile_trace returns None so no CompiledTrace ever
        // dispatches), so the helpers never read those TLS values.
        // JitVmGuard's drop is already a no-op (see the comment on
        // `JitVmGuard::drop` in `mod.rs`).
        crate::jit::noop_jit_guard()
    }
}

impl TraceCompiler for NullJitBackend {
    fn try_compile_trace(&self, _: &TraceRecord, _: CompileOptions) -> Option<CompiledTrace> {
        None
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        "null-backend"
    }
}
