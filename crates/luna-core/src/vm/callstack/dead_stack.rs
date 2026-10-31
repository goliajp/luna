//! A dead coroutine's stack as the VM copies it from its own.

use crate::runtime::function::DeadStack;
use crate::runtime::mem::LVec;
use crate::vm::exec::Vm;

impl Vm {
    /// The running coroutine's frames and natives, copied; `None` without
    /// the memory for the copy (the dead coroutine then shows no levels).
    pub(crate) fn dead_stack(&self) -> Option<DeadStack> {
        let mem = self.heap.mem();
        Some(DeadStack {
            frames: LVec::from_slice(mem, &self.frames).ok()?,
            natives: LVec::from_slice(mem, &self.running_natives[self.natives_base..]).ok()?,
        })
    }
}
