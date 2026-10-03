//! The trace JIT behind the interpreter's `TraceCompiler` hook.

use super::*;

impl TraceCompiler for CraneliftBackend {
    // pass storage through so the compiled trace's `JITModule` is
    // parked on the per-`Vm` `storage.trace_handles` Vec
    fn try_compile_trace(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        trace::compile_trace_for_vm(storage, record, opts, false, None)
    }

    fn try_compile_trace_for(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
        version: luna_core::version::LuaVersion,
    ) -> Option<CompiledTrace> {
        let float_only = version <= luna_core::version::LuaVersion::Lua52;
        trace::compile_trace_for_vm(storage, record, opts, float_only, Some(version))
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        trace::last_compile_checkpoint()
    }

    fn tier_up(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        ct: &CompiledTrace,
    ) -> Option<luna_core::jit::trace::TraceFn> {
        trace::tier_up_trace(storage, ct)
    }

    fn adopt_traces(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        req: &luna_core::jit::trace::AdoptRequest<'_>,
    ) -> Vec<luna_core::jit::trace::AdoptedTrace> {
        trace::adopt_traces(storage, req)
    }
}
