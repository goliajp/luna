//! Installing a JIT backend and its storage on a Vm.

use super::*;

impl Vm {
    /// Install a caller-supplied JIT backend. The
    /// `luna` crate uses this to swap in its `CraneliftBackend`; tests
    /// or third-party backends pass their own [`crate::jit::IntChunkCompiler`] /
    /// [`crate::jit::TraceCompiler`] implementations. A Vm starts with
    /// both JIT flags off; this turns on each flag the embedder has not
    /// set with [`Self::set_jit_enabled`] / [`Self::set_trace_jit_enabled`]
    /// (or [`Self::install_null_jit`]). Re-installing on a Vm whose
    /// closures already populated `Proto.jit: JitProtoState::Compiled`
    /// does NOT evict those cached entries — call right after
    /// construction for a clean swap.
    ///
    /// Naming: `install_jit_backend` (not `install_default_jit`)
    /// because the "default" in luna-core is `NullJitBackend`; the
    /// "default JIT" lives in the `luna` crate.
    pub fn install_jit_backend<C, T>(&mut self, chunk: C, trace: T)
    where
        C: crate::jit::IntChunkCompiler + 'static,
        T: crate::jit::TraceCompiler + 'static,
    {
        self.jit.chunk_compiler = Box::new(chunk);
        self.jit.trace_compiler = Box::new(trace);
        if !self.jit.enabled_chosen {
            self.set_jit_flag(true);
        }
        if !self.jit.trace_enabled_chosen {
            self.jit.trace_enabled = true;
        }
    }

    /// Install a caller-supplied JIT
    /// storage holder. Default is [`crate::jit::NullJitStorage`];
    /// the `luna_jit` crate's `install_default_jit` pairs this with
    /// `install_jit_backend(CraneliftBackend, CraneliftBackend)` to
    /// also install a fresh `CraneliftJitStorage`. Storage holds
    /// the per-`Vm` JIT cache + handle collections.
    ///
    /// The storage it replaces is kept until the Vm drops: functions
    /// compiled through it may still be called.
    pub fn install_jit_storage<S>(&mut self, storage: S)
    where
        S: crate::jit::JitStorage + 'static,
    {
        let old = std::mem::replace(&mut self.jit.storage, Box::new(storage));
        self.retired_jit_storage.push(old);
    }

    /// Install the no-op JIT backend and switch the JIT off
    /// ([`Self::set_jit_enabled`] and [`Self::set_trace_jit_enabled`]
    /// both `false`): no hot counter ticks, no trace is recorded, and
    /// the interpreter skips the per-instruction trace lookup.
    /// Installing a real backend afterwards does not switch the JIT
    /// back on; call the two setters with `true` for that.
    ///
    /// Calling this on a Vm whose closures already populated
    /// `Proto.jit: JitProtoState::Compiled` does NOT evict those
    /// cached entries — the dispatcher will still call into them. For
    /// a truly JIT-free run, call this immediately after construction.
    pub fn install_null_jit(&mut self) {
        self.jit.chunk_compiler = Box::new(crate::jit::NullJitBackend);
        self.jit.trace_compiler = Box::new(crate::jit::NullJitBackend);
        self.set_jit_enabled(false);
        self.set_trace_jit_enabled(false);
    }
}
