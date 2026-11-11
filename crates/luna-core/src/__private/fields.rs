//! Fields of the runtime's objects that luna's other crates read or write.

use crate::jit::{CompiledTrace, TArc};
pub use crate::runtime::function::jit_state::{JIT_NEXT_NONE, JitProtoState};
use crate::runtime::mem::{LAny, LVec};
use crate::runtime::{Coro, Value, function::Proto};
use crate::vm::Vm;
use crate::vm::jit_state::{JitCounters, JitState};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// The JIT's state on the `Vm`. Each method but [`Self::jit_mut`] changes
/// only what cannot break the code the JIT runs: none hands out the
/// storage the code lives in. The example compiles, the two after it fail
/// to.
///
/// ```
/// use luna_core::{__private::VmJitState, version::LuaVersion, vm::Vm};
/// let mut vm = Vm::new(LuaVersion::Lua54);
/// vm.set_trace_hot_threshold(1);
/// vm.jit_counters_mut().deopt += 1;
/// vm.jit_clear_cache();
/// let _ = vm.jit().storage.as_any();
/// // SAFETY: nothing is moved or replaced
/// let _ = unsafe { vm.jit_mut() }.ssa_roots;
/// ```
///
/// ```compile_fail,E0133
/// use luna_core::{__private::VmJitState, version::LuaVersion, vm::Vm};
/// let mut a = Vm::new(LuaVersion::Lua54);
/// let mut b = Vm::new(LuaVersion::Lua54);
/// std::mem::swap(a.jit_mut(), b.jit_mut()); // error[E0133]: `jit_mut` is unsafe
/// ```
///
/// ```compile_fail,E0599
/// use luna_core::{__private::VmJitState, version::LuaVersion, vm::Vm};
/// let mut vm = Vm::new(LuaVersion::Lua54);
/// let _ = vm.jit_storage_mut(); // error[E0599]: no such method
/// ```
pub trait VmJitState {
    /// The JIT's state.
    fn jit(&self) -> &JitState;
    /// What the dispatcher made for trace exits that could not.
    fn jit_unmade_counts(&self) -> crate::vm::exec::UnmadeCounts;
    /// The JIT's state, to change.
    ///
    /// # Safety
    /// Neither the state nor its storage, compilers or recording is moved
    /// or replaced through the reference (as by `std::mem::swap` with
    /// another VM's); the fields the dispatcher and the collector trust
    /// (the recording, the register and root buffers and counts, the
    /// string buffers) are written only as the interpreter writes them.
    unsafe fn jit_mut(&mut self) -> &mut JitState;
    /// Forgets the compiled code the JIT's storage keeps for reuse (see
    /// [`JitStorage::clear_cache`](crate::jit::JitStorage::clear_cache)); code
    /// already in use stays mapped.
    fn jit_clear_cache(&mut self);
    /// The JIT's event counters.
    fn jit_counters_mut(&mut self) -> &mut JitCounters;
    /// Raises a runtime error with `msg` once the running JIT code returns
    /// to the interpreter, which then runs the op again.
    fn park_jit_error(&mut self, msg: &str);
    /// Back-edge visits before a loop is recorded.
    fn set_trace_hot_threshold(&mut self, n: u32);
    /// Calls before a function is recorded or compiled.
    fn set_call_hot_threshold(&mut self, n: u32);
}

impl VmJitState for Vm {
    #[inline(always)]
    fn jit(&self) -> &JitState {
        &self.jit
    }

    fn jit_unmade_counts(&self) -> crate::vm::exec::UnmadeCounts {
        self.unmade.made
    }

    #[inline(always)]
    unsafe fn jit_mut(&mut self) -> &mut JitState {
        &mut self.jit
    }

    fn jit_clear_cache(&mut self) {
        self.jit.storage.clear_cache();
    }

    #[inline(always)]
    fn jit_counters_mut(&mut self) -> &mut JitCounters {
        &mut self.jit.counters
    }

    fn park_jit_error(&mut self, msg: &str) {
        let e = self.runtime_error(msg);
        self.jit.set_pending_err(Some(e));
    }

    fn set_trace_hot_threshold(&mut self, n: u32) {
        self.jit.trace_hot_threshold = n;
    }

    fn set_call_hot_threshold(&mut self, n: u32) {
        self.jit.call_hot_threshold = n;
    }
}

/// What the C API keeps on a thread.
pub trait CoroHostInternals {
    /// The thread's stack as the C API sees it (PUC `lua_State`'s stack).
    fn host_stack(&self) -> &LVec<Value>;
    /// The thread's C API stack, to change.
    fn host_stack_mut(&mut self) -> &mut LVec<Value>;
    /// The C API's `lua_State` for the thread, once C has asked for one.
    fn host_state(&self) -> Option<&LAny>;
    /// Keeps `state` as the thread's `lua_State` for as long as the thread
    /// lives.
    fn set_host_state(&mut self, state: LAny);
}

impl CoroHostInternals for Coro {
    #[inline]
    fn host_stack(&self) -> &LVec<Value> {
        &self.host_stack
    }

    #[inline]
    fn host_stack_mut(&mut self) -> &mut LVec<Value> {
        &mut self.host_stack
    }

    #[inline]
    fn host_state(&self) -> Option<&LAny> {
        self.host_state.as_ref()
    }

    #[inline]
    fn set_host_state(&mut self, state: LAny) {
        self.host_state = Some(state);
    }
}

/// An unused slot of [`ProtoTraceInternals::trace_heads`].
pub const TRACE_HEADS_NONE: u32 = crate::runtime::function::TRACE_HEADS_NONE;

/// Where a prototype's traces can be entered.
pub trait ProtoTraceInternals {
    /// Whether the prototype holds a trace the dispatcher may enter at its
    /// head.
    fn has_dispatchable_trace(&self) -> bool;
    /// The head pcs of those traces, the unused slots [`TRACE_HEADS_NONE`].
    fn trace_heads(&self) -> Vec<u32>;
    /// Whether the call trigger is done with the prototype's entry.
    fn trace_call_head_settled(&self) -> bool;
    /// Per slot of [`Self::trace_heads`]: what the fast loop notes about
    /// entries the traces there refuse, `u32::MAX` when nothing.
    fn trace_head_refusals(&self) -> Vec<u32>;
    /// The method JIT's state of the prototype.
    fn jit_state(&self) -> JitProtoState;
    /// The prototype's compiled traces. Nothing in them can be changed
    /// through the reference.
    fn traces(&self) -> impl std::ops::Deref<Target = Vec<TArc<CompiledTrace>>> + '_;
    /// Takes the code a backend is compiling for the prototype in the
    /// background, leaving none pending.
    fn take_jit_next(&self) -> Option<Arc<AtomicUsize>>;
    /// Sets the code a backend is compiling for the prototype in the
    /// background.
    ///
    /// # Safety
    /// The number the cell holds, now and whenever it changes, is 0,
    /// [`JIT_NEXT_NONE`], or the entry of live code with the ABI of the
    /// prototype's `JitProtoState::Compiled` entry, which the next call
    /// from the interpreter installs and calls.
    unsafe fn set_jit_next(&self, next: Option<Arc<AtomicUsize>>);
}

impl ProtoTraceInternals for Proto {
    fn has_dispatchable_trace(&self) -> bool {
        self.has_dispatchable_trace.get()
    }

    fn trace_heads(&self) -> Vec<u32> {
        self.trace_heads.get().to_vec()
    }

    fn trace_call_head_settled(&self) -> bool {
        self.trace_call_head_settled.get()
    }

    fn trace_head_refusals(&self) -> Vec<u32> {
        self.trace_head_refused
            .iter()
            .map(std::cell::Cell::get)
            .collect()
    }

    fn jit_state(&self) -> JitProtoState {
        self.jit.get()
    }

    fn traces(&self) -> impl std::ops::Deref<Target = Vec<TArc<CompiledTrace>>> + '_ {
        self.traces.borrow()
    }

    fn take_jit_next(&self) -> Option<Arc<AtomicUsize>> {
        self.jit_next.take()
    }

    unsafe fn set_jit_next(&self, next: Option<Arc<AtomicUsize>>) {
        self.jit_next.set(next);
    }
}
