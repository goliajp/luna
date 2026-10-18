//! The main thread's call stack while a coroutine it resumed runs.

use super::*;
use crate::runtime::mem::LVec;
use crate::vm::callstack::ThreadStack;

impl Vm {
    /// The main thread's levels from its saved context: it waits on the
    /// coroutines it resumed, so its natives end where the first of them
    /// starts.
    pub(crate) fn main_thread_stack(&self) -> ThreadStack<'_> {
        let m = self
            .main_ctx
            .as_ref()
            .expect("the main thread's saved context");
        let mut end = self.natives_base;
        let mut c = self.current;
        while let Some(co) = c {
            if co.resumer.is_none() {
                break;
            }
            c = co.resumer;
            if let Some(r) = c {
                end = r.natives.start;
            }
        }
        ThreadStack::new(
            self.version <= LuaVersion::Lua51,
            &m.frames,
            &m.stack,
            m.top,
            &self.running_natives[..end],
            None,
        )
        .suspended()
    }

    /// The main thread's saved stack, written by `debug.setlocal` from a
    /// coroutine it resumed.
    pub(crate) fn main_stack_mut(&mut self) -> &mut LVec<Value> {
        &mut self
            .main_ctx
            .as_mut()
            .expect("the main thread's saved context")
            .stack
    }
}
