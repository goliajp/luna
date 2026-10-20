//! The stack a coroutine an error killed keeps: its frames and the natives
//! running in it where the error was raised, as PUC leaves a dead thread's
//! `CallInfo`s in place. Levels are made from it only when asked for.

use super::NativeAct;
use crate::runtime::function::CallFrame;
use crate::runtime::heap::Marker;
use crate::runtime::mem::LVec;
use crate::runtime::{ContKind, Value};
use crate::vm::exec::Vm;

/// A dead coroutine's frames and running natives.
pub(crate) struct DeadStack {
    pub(crate) frames: LVec<CallFrame>,
    pub(crate) natives: LVec<NativeAct>,
}

impl DeadStack {
    /// What the frames and natives hold: the functions running in them and
    /// the message handlers of their xpcalls.
    pub(crate) fn trace(&self, m: &mut Marker) {
        for cf in self.frames.iter() {
            match cf {
                CallFrame::Lua(f) => {
                    m.mark(f.closure);
                }
                CallFrame::Cont(nc) => {
                    if let ContKind::Xpcall { handler, .. } = nc.kind {
                        m.value(handler);
                    }
                }
            }
        }
        for a in self.natives.iter() {
            m.value(Value::Native(a.nc));
        }
    }
}

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
