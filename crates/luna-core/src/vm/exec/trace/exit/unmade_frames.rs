//! The inlined frames a trace's inline exit could not push, pushed by the
//! dispatcher before the write-back.

use super::*;
use crate::jit::trace::{CompiledTrace, ExitTag, FrameMaterializeInfo};

impl Vm {
    /// Before the write-back fills their registers: the frames the inline
    /// exit `site_id` of `trace` could not push (see
    /// [`Self::trace_push_unmade_frames`]), its registers from `off` of the
    /// window of the trace whose head frame is at `base` and has
    /// `max_stack` registers.
    #[inline(always)]
    pub(in crate::vm::exec) fn unmade_frames_at(
        &mut self,
        (trace, site_id): (&CompiledTrace, u32),
        tags: (&[ExitTag], Option<&[u8]>),
        (off, base, max_stack): (usize, usize, usize),
        reg_state: &[i64],
    ) -> Option<(LuaError, u32)> {
        if !self.unmade.any() || self.unmade.frames.is_none() || site_id == 0 {
            return None;
        }
        let view = UnmadeFramesView::of(trace, site_id, tags, off, base, max_stack);
        self.trace_push_unmade_frames(&view, reg_state)
    }

    /// Before the registers of the exit `view` describes are written back:
    /// the inlined frames the exit could not push, pushed as the
    /// interpreter makes a call's frame. The registers are rooted
    /// meanwhile, since an emergency collection may run. `Some` with the
    /// memory error and the pc the frame left on top resumes at when one
    /// cannot be made: the error is the call's into that frame.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_push_unmade_frames(
        &mut self,
        view: &UnmadeFramesView<'_>,
        reg_state: &[i64],
    ) -> Option<(LuaError, u32)> {
        let pushed = self.unmade.frames.take()? as usize;
        self.unmade.made.frames += 1;
        let w0 = self.unmade.roots.len();
        self.root_exit_window(view, reg_state);
        // an emergency collection keeps the head frame's registers, which
        // the roots take from the stack
        let end = (view.head_base + view.max_stack) as u32;
        let top = self.top;
        self.top = top.max(end);
        let mut failed = None;
        for (j, m) in view.chain.iter().enumerate().skip(pushed) {
            let slot = w0 + view.off + (m.base_offset - m.n_varargs) as usize - 1;
            let Value::Closure(cl) = self.unmade.roots[slot] else {
                unreachable!("an inlined frame runs the closure its caller called")
            };
            let base = view.base as u32 + m.base_offset;
            let need = (base + u32::from(cl.proto.max_stack)) as usize;
            let room = self.retry_oom(|vm| {
                if vm.stack.len() < need {
                    vm.stack.resize(need, Value::Nil)?;
                }
                vm.frames.reserve(1)
            });
            if let Err(e) = room {
                let pc = match j {
                    0 => view.resume_pc,
                    _ => view.chain[j - 1].pc,
                };
                failed = Some((e, pc));
                break;
            }
            let made = self.jit_push_inlined_frame(cl, base, m.pc, m.nresults, m.n_varargs);
            debug_assert!(made, "the frame stack has room");
        }
        self.top = top;
        self.unmade.roots.truncate(w0);
        failed
    }

    /// The registers of `view`'s window as values, onto the roots: the ones
    /// the exit wrote by their tags, the head frame's others from the
    /// stack, nil past it (as the write-back leaves them).
    fn root_exit_window(&mut self, view: &UnmadeFramesView<'_>, reg_state: &[i64]) {
        let n = view.tags.len().min(reg_state.len());
        for i in 0..n {
            let v = match super::super::restore::exit_value(view.tags[i], reg_state[i]) {
                // a sunk table or the accumulated string the exit could
                // not make either, made after the write-back
                Some(Value::Table(t)) if t.as_ptr() as usize & 1 == 1 => Value::Nil,
                Some(Value::Str(s)) if s.as_ptr().is_null() => Value::Nil,
                Some(v) => v,
                None if i < view.max_stack => match view.entry_tags.map(|e| e[i]) {
                    // a side trace ran on the trace's registers
                    Some(t) if t != crate::jit::trace::ENTRY_TAG_ANY => {
                        super::super::restore::raw_value(t, reg_state[i])
                    }
                    _ => self.stack[view.head_base + i],
                },
                None => Value::Nil,
            };
            super::unmade::push_root(&mut self.unmade.roots, v);
        }
    }
}

/// The exit whose inlined frames are left to push: its frame chain and
/// tags, from register `off` of the trace's window (frame `base` on the
/// stack), the trace's head frame at `head_base` with `max_stack`
/// registers, and the pc the frame below the first resumes at.
pub(in crate::vm::exec) struct UnmadeFramesView<'a> {
    pub(in crate::vm::exec) chain: &'a [FrameMaterializeInfo],
    pub(in crate::vm::exec) tags: &'a [ExitTag],
    pub(in crate::vm::exec) off: usize,
    pub(in crate::vm::exec) base: usize,
    pub(in crate::vm::exec) head_base: usize,
    pub(in crate::vm::exec) max_stack: usize,
    pub(in crate::vm::exec) resume_pc: u32,
    /// The head frame's entry tags, when a side trace ran on the trace's
    /// registers: what it left untouched holds a value of that tag.
    pub(in crate::vm::exec) entry_tags: Option<&'a [u8]>,
}

impl<'a> UnmadeFramesView<'a> {
    /// The inline exit `site_id` of `ct` (`> 0`), its registers from `off`
    /// of the window of the trace whose head frame is at `head_base`.
    pub(in crate::vm::exec) fn of(
        ct: &'a CompiledTrace,
        site_id: u32,
        (tags, entry_tags): (&'a [ExitTag], Option<&'a [u8]>),
        off: usize,
        head_base: usize,
        max_stack: usize,
    ) -> Self {
        let site = &ct.per_exit_inline[(site_id - 1) as usize];
        UnmadeFramesView {
            chain: &site.chain,
            tags,
            off,
            base: head_base + off,
            head_base,
            max_stack,
            resume_pc: site.head_resume_pc,
            entry_tags,
        }
    }
}
