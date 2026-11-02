//! After a trace leaves for a collection, the interpreter runs on to where
//! that trace is entered again without counting anything hot: when the
//! collector's steps fall due depends on how much the program allocated,
//! so what it runs then would make which traces form depend on it too.

use super::*;
use crate::runtime::Proto;

/// The trace a collection exit left: the frame it ran in (its depth, base
/// and function) and its head.
#[derive(Clone, Copy)]
pub(crate) struct GcQuiet {
    depth: usize,
    base: u32,
    proto: Gc<Proto>,
    head_pc: u32,
}

impl Vm {
    /// A trace with head `head_pc`, run in the frame `pre_frames` deep, left
    /// for a collection: hot counting pauses.
    pub(in crate::vm::exec) fn gc_quiet_from(&mut self, pre_frames: usize, head_pc: u32) {
        let Some(CallFrame::Lua(f)) = pre_frames.checked_sub(1).and_then(|i| self.frames.get(i))
        else {
            return;
        };
        self.jit.step.quiet = Some(GcQuiet {
            depth: pre_frames,
            base: f.base,
            proto: f.closure.proto,
            head_pc,
        });
    }

    /// Whether a back-edge to, or a call reaching, `pc` of `proto` counts
    /// toward recording nothing: until the interpreter enters a trace,
    /// returns from the frame the collection exit left, or reaches that
    /// trace's head there, whichever comes first.
    #[inline]
    pub(in crate::vm::exec) fn hot_paused_at(&mut self, proto: Gc<Proto>, pc: u32) -> bool {
        match self.jit.step.quiet {
            None => false,
            Some(q) => self.hot_paused_slow(q, proto, pc),
        }
    }

    #[cold]
    fn hot_paused_slow(&mut self, q: GcQuiet, proto: Gc<Proto>, pc: u32) -> bool {
        let same_frame = matches!(
            self.frames.get(q.depth - 1),
            Some(CallFrame::Lua(f)) if f.base == q.base && f.closure.proto.ptr_eq(q.proto)
        );
        let at_head = proto.ptr_eq(q.proto) && pc == q.head_pc;
        if self.frames.len() < q.depth || !same_frame || at_head {
            self.jit.step.quiet = None;
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests;
