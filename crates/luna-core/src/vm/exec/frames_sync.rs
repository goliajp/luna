//! Pushing and popping frames with the bookkeeping that goes with it.

use super::*;

// Split-borrow free fn helpers for frames push/pop with shadow counter
// `frames_top: u32`. Free fns (not Vm methods) so callers can pass
// `&mut self.frames` + `&mut self.frames_top` as split borrows, allowing
// other `&mut self.field` reads inside the CallFrame construction (e.g.
// `std::mem::take(&mut self.pending_tm)`).
//
// The shadow has no readers yet; it just stays in sync + asserts.
//
// `trap` is the dispatch loop's slow-path flag: a continuation frame on top
// of the stack must be seen by the loop head, which tests nothing else
// unless `trap` is set. So pushing a continuation sets it (the protected
// call may finish without a frame of its own), and so does a pop that
// leaves one on top.
#[inline(always)]
pub(super) fn frames_push_sync(
    frames: &mut Vec<CallFrame>,
    frames_top: &mut u32,
    trap: &mut bool,
    cf: CallFrame,
) {
    if matches!(cf, CallFrame::Cont(_)) {
        *trap = true;
    }
    frames.push(cf);
    // Shadow maintenance is debug-only: release builds skip the
    // increment + assertion entirely. While nothing reads the shadow,
    // its purpose is to VERIFY the assumed invariant
    // (frames_top == frames.len()) across all push/pop sites; once readers
    // consume it, release must run the increment unconditionally.
    #[cfg(debug_assertions)]
    {
        *frames_top += 1;
        debug_assert_eq!(
            *frames_top as usize,
            frames.len(),
            "P17-D frames_top out of sync after push",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = frames_top;
}

#[inline(always)]
pub(super) fn frames_pop_sync(
    frames: &mut Vec<CallFrame>,
    frames_top: &mut u32,
    trap: &mut bool,
) -> Option<CallFrame> {
    let r = frames.pop();
    if matches!(frames.last(), Some(CallFrame::Cont(_))) {
        *trap = true;
    }
    #[cfg(debug_assertions)]
    {
        if r.is_some() {
            *frames_top = frames_top.saturating_sub(1);
        }
        debug_assert_eq!(
            *frames_top as usize,
            frames.len(),
            "P17-D frames_top out of sync after pop",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = frames_top;
    r
}

/// [`frames_pop_sync`] when the caller knows the frame below the top one
/// and whether it is a continuation: `frames.len() >= 2`.
#[inline(always)]
pub(super) fn frames_pop_known(
    frames: &mut Vec<CallFrame>,
    frames_top: &mut u32,
    trap: &mut bool,
    cont_below: bool,
) {
    debug_assert!(frames.len() >= 2);
    debug_assert_eq!(
        cont_below,
        matches!(frames[frames.len() - 2], CallFrame::Cont(_))
    );
    frames.truncate(frames.len() - 1);
    if cont_below {
        *trap = true;
    }
    #[cfg(debug_assertions)]
    {
        *frames_top = frames_top.saturating_sub(1);
        debug_assert_eq!(*frames_top as usize, frames.len());
    }
    #[cfg(not(debug_assertions))]
    let _ = frames_top;
}

impl Vm {
    /// `trap` is set after a call or return: when that is only because a
    /// metamethod call pushed its continuation, or a metamethod returned to
    /// one, finish what the loop head would and report whether a Lua frame
    /// with nothing to watch is now on top. Anything else (a hook, a budget,
    /// a memory cap, another kind of continuation) is left to the loop head.
    #[inline(never)]
    pub(super) fn settle_frames(&mut self, entry_depth: usize) -> Result<bool, LuaError> {
        if self.instr_budget.is_some() || self.heap.mem_cap.is_some() || self.hook_armed() {
            return Ok(false);
        }
        loop {
            match self.frames.last() {
                Some(CallFrame::Lua(_)) => {
                    self.trap = false;
                    return Ok(true);
                }
                Some(&CallFrame::Cont(nc)) if matches!(nc.kind, ContKind::Meta(_)) => {
                    // a metamethod's result completes the instruction; this
                    // kind never hands results out of the activation
                    let out = self.finish_cont(nc, entry_depth)?;
                    debug_assert!(out.is_none());
                }
                _ => return Ok(false),
            }
        }
    }
}

/// A metamethod, `__pairs` or `__close` continuation: one C level in PUC,
/// counted in `pcall_depth` while it is on the frame stack.
#[inline]
pub(super) fn is_c_level_cont(f: &CallFrame) -> bool {
    matches!(
        f,
        CallFrame::Cont(NativeCont {
            kind: ContKind::Meta(_) | ContKind::Pairs | ContKind::Close(_),
            ..
        })
    )
}

impl Vm {
    /// Pop the top frame, giving back the C level a continuation held.
    pub(super) fn pop_frame(&mut self) {
        if let Some(f) = frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap)
            && is_c_level_cont(&f)
        {
            self.pcall_depth -= 1;
        }
    }
}
