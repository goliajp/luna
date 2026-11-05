//! After a trace leaves for a collection, the interpreter runs on to where
//! that trace is entered again without counting anything hot: when the
//! collector's steps fall due depends on how much the program allocated,
//! so what it runs then would make which traces form depend on it too.
//!
//! The back-edges pay nothing for this outside a pause: while one lasts,
//! the loop counts are parked and a stand-in table whose every slot is at
//! the threshold takes their place, so that each back-edge reaches
//! [`Vm::back_edge_paused`], which counts nothing or ends the pause.

use super::*;
use crate::runtime::Proto;
use crate::vm::jit_state::LOOP_HOT_SLOTS;

/// A table of loop counts ([`crate::vm::JitState::loop_hot`]).
pub(crate) type HotTable = Box<[u32; LOOP_HOT_SLOTS]>;

/// The trace a collection exit left: the frame it ran in (its depth, base
/// and function), its head, the last back-edge to that head (`None` when
/// no back-edge of the function reaches it; found when first asked, as a
/// collection exit is frequent and the question rare), and the back-edges
/// the pause has held back so far.
#[derive(Clone, Copy)]
pub(crate) struct GcQuiet {
    depth: usize,
    base: u32,
    proto: Gc<Proto>,
    head_pc: u32,
    loop_end: Option<Option<u32>>,
    held: u32,
}

/// Where the back-edge `inst` at `pc` goes, if it is one.
fn back_edge_target(inst: crate::vm::isa::Inst, pc: u32) -> Option<u32> {
    let op = inst.op();
    let off = if op.is_for_loop() || op.is_tfor_loop() {
        -(inst.bx() as i64)
    } else if matches!(op, Op::Jmp | Op::JmpClose | Op::JmpCloseBack) {
        i64::from(inst.jump_offset())
    } else {
        return None;
    };
    (off < 0).then(|| (i64::from(pc) + 1 + off).max(0) as u32)
}

/// The pc of the last back-edge of `proto` that goes to `head`: the trace's
/// loop runs from `head` to there.
fn loop_end(proto: Gc<Proto>, head: u32) -> Option<u32> {
    let code = &proto.code;
    (0..code.len() as u32)
        .rev()
        .find(|&pc| back_edge_target(code[pc as usize], pc) == Some(head))
}

impl Vm {
    /// A trace with head `head_pc`, run in the frame `pre_frames` deep, left
    /// for a collection: hot counting pauses.
    pub(in crate::vm::exec) fn gc_quiet_from(&mut self, pre_frames: usize, head_pc: u32) {
        let Some(CallFrame::Lua(f)) = pre_frames.checked_sub(1).and_then(|i| self.frames.get(i))
        else {
            return;
        };
        let q = GcQuiet {
            depth: pre_frames,
            base: f.base,
            proto: f.closure.proto,
            head_pc,
            loop_end: None,
            held: 0,
        };
        let step = &mut self.jit.step;
        if step.quiet.replace(q).is_none() {
            let stand_in = step
                .parked_hot
                .take()
                .unwrap_or_else(|| Box::new([u32::MAX; LOOP_HOT_SLOTS]));
            step.parked_hot = Some(std::mem::replace(&mut self.jit.loop_hot, stand_in));
        }
    }

    /// Ends the pause: the loop counts are back.
    #[cold]
    pub(in crate::vm::exec) fn gc_quiet_end(&mut self) {
        let step = &mut self.jit.step;
        step.quiet = None;
        let counts = step.parked_hot.take().expect("parked during a pause");
        step.parked_hot = Some(std::mem::replace(&mut self.jit.loop_hot, counts));
    }

    /// A back-edge to `target` of `proto` reached the threshold while a
    /// pause may last: `true` when it counts and is hot. During a pause
    /// that only happens through the stand-in table; the back-edge counts
    /// nothing unless it ends the pause.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn back_edge_paused(&mut self, proto: Gc<Proto>, target: u32) -> bool {
        // the stand-in slot the back-edge just started over
        self.jit.loop_hot.fill(u32::MAX);
        let paused = match self.jit.step.quiet {
            None => false,
            Some(q) => self.hot_paused_slow(q, proto, target, true),
        };
        if paused {
            // nor in the function's count, which `back_edge_hot` raised
            let c = proto.trace_hot_count.get();
            proto.trace_hot_count.set(c.saturating_sub(1));
            // as many back-edges held back as make a loop hot end the
            // pause: a long call the trace's loop makes counts again
            let threshold = self.jit.trace_hot_threshold;
            if let Some(q) = self.jit.step.quiet.as_mut() {
                q.held += 1;
                if q.held >= threshold {
                    self.gc_quiet_end();
                }
            }
            return false;
        }
        self.jit.loop_hot_tick(&proto, target)
    }

    /// Whether a back-edge to, or a call reaching, `pc` of `proto` counts
    /// toward recording nothing: until the interpreter enters a trace,
    /// returns from the frame the collection exit left, reaches that
    /// trace's head there, takes another back-edge there, calls from there
    /// outside the trace's loop, or has held back as many back-edges as
    /// make a loop hot, whichever comes first.
    #[inline]
    pub(in crate::vm::exec) fn hot_paused_at(&mut self, proto: Gc<Proto>, pc: u32) -> bool {
        match self.jit.step.quiet {
            None => false,
            Some(q) => self.hot_paused_slow(q, proto, pc, false),
        }
    }

    /// A back-edge (`back_edge`) in the frame the exit left that goes
    /// anywhere but the trace's head also ends the pause: a depth-0 loop
    /// ends a trace, so no other back-edge of that frame lies inside it,
    /// and reaching one means the trace's loop is over and the interpreter
    /// runs what it would have run after the trace left normally. So does
    /// a call that frame makes from outside the trace's loop.
    #[cold]
    fn hot_paused_slow(&mut self, q: GcQuiet, proto: Gc<Proto>, pc: u32, back_edge: bool) -> bool {
        let same_frame = matches!(
            self.frames.get(q.depth - 1),
            Some(CallFrame::Lua(f)) if f.base == q.base && f.closure.proto.ptr_eq(q.proto)
        );
        let in_frame = same_frame && self.frames.len() == q.depth;
        let at_head = proto.ptr_eq(q.proto) && pc == q.head_pc;
        // a call the frame makes: its pc is already past the call
        let calls = !back_edge && same_frame && self.frames.len() == q.depth + 1;
        let end = if calls { self.quiet_loop_end(q) } else { None };
        let called_after_loop = calls
            && !self.frames[q.depth - 1].lua().is_some_and(|f| {
                let call_pc = f.pc.wrapping_sub(1);
                end.is_some_and(|end| (q.head_pc..=end).contains(&call_pc))
            });
        if self.frames.len() < q.depth
            || !same_frame
            || at_head
            || (back_edge && in_frame)
            || called_after_loop
        {
            self.gc_quiet_end();
            return false;
        }
        true
    }
}

impl Vm {
    /// `q`'s last back-edge to its head, found once per pause.
    fn quiet_loop_end(&mut self, q: GcQuiet) -> Option<u32> {
        if let Some(end) = q.loop_end {
            return end;
        }
        let end = loop_end(q.proto, q.head_pc);
        if let Some(p) = self.jit.step.quiet.as_mut() {
            p.loop_end = Some(end);
        }
        end
    }
}

#[cfg(test)]
mod tests;
