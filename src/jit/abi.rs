//! v1.1 A1 Session A — JIT-backend trait boundary (in-place).
//!
//! Defines the `IntChunkCompiler` + `TraceCompiler` traits that the
//! interpreter dispatcher in `vm/exec.rs` routes through to invoke the
//! JIT codegen. Two implementations live next to it:
//!
//! - [`NullJitBackend`] — does nothing (interp-only embedders).
//! - [`CraneliftBackend`] — delegates to the existing free functions
//!   in `crate::jit` (`cache_lookup_or_compile`,
//!   `try_compile_trace_with_options`, `enter_jit`,
//!   `last_compile_checkpoint`).
//!
//! For Session A the file layout is unchanged: this just introduces the
//! trait surface so `Vm` dispatches via `Box<dyn IntChunkCompiler>` /
//! `Box<dyn TraceCompiler>`. The directory split into `luna-core` /
//! `luna` (RFC Steps 4-11) happens in a later session.
//!
//! RFC: `.dev/rfcs/v1.1-rfc-crate-split.md` §D1 + Migration steps 1-3.

use crate::jit::JitVmGuard;
use crate::jit::trace::{CompileOptions, CompiledTrace, TraceRecord};
use crate::runtime::Gc;
use crate::runtime::LuaClosure;
use crate::runtime::function::Proto;

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

/// No-op backend installed when an embedder wants the interpreter
/// without paying any JIT cost. Both calls report "nothing compiled"
/// so the interp dispatcher always takes the standard path.
///
/// Bare `Vm::new_minimal` still installs the real Cranelift backend
/// in Session A (v1.0 behavior preserved); tests opt into the null
/// backend via [`crate::vm::Vm::install_null_jit`].
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
        // `JitVmGuard::drop` in `src/jit/mod.rs`).
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

/// Default Cranelift-backed JIT. Delegates to the existing free
/// functions in `crate::jit` — for Session A these still live where
/// they always did; in later RFC steps the bodies move to
/// `crates/luna/src/jit_backend/`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CraneliftBackend;

impl IntChunkCompiler for CraneliftBackend {
    fn try_compile(&self, proto: Gc<Proto>, pre53: bool, float_only: bool) -> CompileResult {
        match crate::jit::cache_lookup_or_compile(proto, pre53, float_only) {
            Some((
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            )) => CompileResult::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            },
            None => CompileResult::Skipped,
        }
    }

    fn enter(&self, vm: *mut crate::vm::Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
        // SAFETY: the dispatcher derived `vm` from a live `&mut Vm`
        // and the JIT entry that runs under this guard does not
        // re-enter Rust against `Vm` except through the TLS pointer
        // this call installs (helpers reach Vm via `JIT_VM`). Vm is
        // `?Send` / single-threaded. The raw-ptr indirection here
        // only sidesteps the lexical borrow conflict against
        // `self.chunk_compiler`.
        let vm_ref: &mut crate::vm::Vm = unsafe { &mut *vm };
        crate::jit::enter_jit(vm_ref, cl)
    }
}

impl TraceCompiler for CraneliftBackend {
    fn try_compile_trace(
        &self,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        crate::jit::trace::try_compile_trace_with_options(record, opts)
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        crate::jit::trace::last_compile_checkpoint()
    }
}
