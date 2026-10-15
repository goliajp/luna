//! What a thread's frames reserve of its stack (PUC `stackinuse`), and the
//! stack shrunk to it.

use super::stack_size;
use super::*;
use crate::vm::callstack::NativeAct;

/// PUC's `ci->top` of a thread's bottom `CallInfo`, in luna's numbering:
/// `LUA_MINSTACK` above its empty function slot.
const BASE_CI_TOP: u32 = stack_size::MIN_STACK;

/// PUC `stackinuse` of a thread with these frames, top and running natives:
/// one past the highest slot the top or a frame's reserved stack reaches,
/// in PUC's numbering.
fn thread_in_use(frames: &[CallFrame], top: u32, natives: &[NativeAct]) -> u32 {
    let mut lim = top.max(BASE_CI_TOP);
    for f in frames {
        lim = lim.max(match f {
            CallFrame::Lua(f) => f.base + u32::from(f.closure.proto.max_stack),
            CallFrame::Cont(c) => c.kind.ci_top(),
        });
    }
    for a in natives {
        lim = lim.max(a.ci_top);
    }
    lim + 2
}

impl Vm {
    /// [`thread_in_use`] of the running thread.
    pub(super) fn stack_in_use(&self) -> u32 {
        thread_in_use(
            &self.frames,
            self.top,
            &self.running_natives[self.natives_base..],
        )
    }

    /// PUC's collector visiting the running thread and the main thread it
    /// suspended (`traversethread` → `luaD_shrinkstack`; 5.1
    /// `checkstacksizes`, which halves the `CallInfo` array too). When the
    /// collector runs is its own (luna's cycles do not start where PUC's
    /// do), and a suspended coroutine keeps its size.
    pub(super) fn collector_shrinks_stacks(&mut self) {
        let v = self.version;
        let inuse = self.stack_in_use();
        if v == LuaVersion::Lua51 {
            // the host collecting with no call running has only the base
            let used = self.frames_in_use().saturating_sub(1);
            let (s, calls) = halve_51(self.stack_size, inuse, self.g.frame_size, used);
            self.set_stack_size(s);
            self.g.frame_size = calls;
        } else {
            self.set_stack_size(stack_size::shrink(v, self.stack_size, inuse));
        }
        let host_frames = self.g.host_frames;
        let natives = &self.running_natives;
        if let Some(m) = &mut self.main_ctx {
            let inuse = thread_in_use(&m.frames, m.top, &natives[m.natives.clone()]);
            if v == LuaVersion::Lua51 {
                let n = m.frames.len() + m.natives.len();
                let used = n as u32 + host_frames - m.meta_conts - 1;
                (m.stack_size, m.frame_size) = halve_51(m.stack_size, inuse, m.frame_size, used);
            } else {
                m.stack_size = stack_size::shrink(v, m.stack_size, inuse);
            }
        }
    }
}

/// PUC 5.1 `checkstacksizes`: the `CallInfo` array of `calls` entries,
/// `used` of them in use, and the stack of size `s`, `inuse` of it used,
/// each halved when less than a quarter is used; neither is touched while
/// the array is past the call limit (an overflow is being handled).
fn halve_51(s: u32, inuse: u32, calls: u32, used: u32) -> (u32, u32) {
    if calls > MAX_CALLS_51 {
        return (s, calls);
    }
    let calls = stack_size::halve_calls_51(calls, used, MAX_CALLS_51, BASIC_FRAME_SIZE_51);
    // `traversestack` measures to the highest slot, not one past it
    (stack_size::halve_51(s, inuse - 1), calls)
}
