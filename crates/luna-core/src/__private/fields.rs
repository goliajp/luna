//! Fields of the runtime's objects that luna's other crates read or write.

use crate::runtime::mem::{LAny, LVec};
use crate::runtime::{Coro, Value, function::Proto};
use crate::vm::Vm;
use crate::vm::jit_state::JitState;

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
}
