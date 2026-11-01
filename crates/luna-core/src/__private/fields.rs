//! Fields of the runtime's objects that luna's other crates read or write.

use crate::jit::{CompiledTrace, TArc};
pub use crate::runtime::function::jit_state::{JIT_NEXT_NONE, JitProtoState};
use crate::runtime::mem::{LAny, LVec};
use crate::runtime::{Coro, Value, function::Proto};
use crate::vm::Vm;
use crate::vm::jit_state::JitState;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// The JIT's state on the `Vm`.
pub trait VmJitState {
    /// The JIT's state.
    fn jit(&self) -> &JitState;
    /// The JIT's state, to change.
    fn jit_mut(&mut self) -> &mut JitState;
}

impl VmJitState for Vm {
    #[inline(always)]
    fn jit(&self) -> &JitState {
        &self.jit
    }

    #[inline(always)]
    fn jit_mut(&mut self) -> &mut JitState {
        &mut self.jit
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
