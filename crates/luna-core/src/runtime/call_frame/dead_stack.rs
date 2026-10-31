//! The stack a coroutine an error killed keeps: its frames and the natives
//! running in it where the error was raised, as PUC leaves a dead thread's
//! `CallInfo`s in place. Levels are made from it only when asked for.

use super::{CallFrame, ContKind, NativeAct};
use crate::runtime::heap::Marker;
use crate::runtime::mem::LVec;
use crate::runtime::value::Value;

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
