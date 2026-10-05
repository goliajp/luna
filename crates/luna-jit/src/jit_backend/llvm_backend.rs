//! The LLVM backend (`LUNA_JIT_BACKEND=llvm`): luna-jit-llvm's method JIT,
//! and the shared trace lowering with LLVM as its optimizing tier.

use luna_core::jit::trace::{CompileOptions, CompiledTrace, TraceFn, TraceRecord};
use luna_core::jit::{CompileResult, IntChunkCompiler, JitStorage, JitVmGuard, TraceCompiler};

/// The LLVM backend. Installed with a storage from
/// `CraneliftJitStorage::with_llvm`: traces start in the baseline tier,
/// as with the Cranelift backend, and LLVM compiles the hot ones
/// (`LUNA_TRACE_TIER=optimizing`: every trace).
#[derive(Clone, Copy, Debug)]
pub struct LlvmBackend {
    /// Compile hot baseline traces on a thread of their own while the
    /// baseline code keeps running (the default); `false` compiles them
    /// when they become hot, before the trace runs on.
    pub background_tier_up: bool,
}

impl Default for LlvmBackend {
    fn default() -> LlvmBackend {
        LlvmBackend {
            background_tier_up: true,
        }
    }
}

impl IntChunkCompiler for LlvmBackend {
    fn try_compile(
        &self,
        storage: &mut dyn JitStorage,
        proto: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
        pre53: bool,
        float_only: bool,
    ) -> CompileResult {
        let Some(llvm) = super::storage::from_storage(storage)
            .ok()
            .and_then(|cs| cs.llvm.as_mut())
        else {
            return CompileResult::Skipped;
        };
        luna_jit_llvm::LlvmBackend.try_compile(llvm, proto, pre53, float_only)
    }

    fn enter(
        &self,
        vm: *mut luna_core::vm::Vm,
        cl: Option<luna_core::runtime::Gc<luna_core::runtime::LuaClosure>>,
    ) -> JitVmGuard {
        luna_jit_helpers::enter_jit_ptr(vm, cl)
    }
}

impl TraceCompiler for LlvmBackend {
    fn try_compile_trace(
        &self,
        storage: &mut dyn JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        super::trace::compile_trace_for_vm(storage, record, opts, false, None)
    }

    fn try_compile_trace_for(
        &self,
        storage: &mut dyn JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
        version: luna_core::version::LuaVersion,
    ) -> Option<CompiledTrace> {
        let float_only = version <= luna_core::version::LuaVersion::Lua52;
        super::trace::compile_trace_for_vm(storage, record, opts, float_only, Some(version))
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        super::trace::last_compile_checkpoint()
    }

    fn tier_up(&self, storage: &mut dyn JitStorage, ct: &CompiledTrace) -> Option<TraceFn> {
        super::trace::tier_up_llvm(storage, ct, self.background_tier_up)
    }
}
