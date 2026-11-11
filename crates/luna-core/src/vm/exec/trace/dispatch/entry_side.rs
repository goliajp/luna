//! An entry the root trace at a head refused: tried on the root's entry
//! side traces, else noted for the fast loop to pass the head by.

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace::CompiledTrace;
use crate::jit::trace_types::installed::entry_side::{MAX_MISSES, NO_LAST};

impl Vm {
    /// After `root`, tried at head `pc` of `cl` (frame `base`), refused the
    /// entry by `refusal` (see [`Vm::trace_marshal_in`]): takes back the
    /// count the entry made towards its move to the optimizing tier, so that
    /// entries that never run do not have it compile code for them, then
    /// puts in `entered` (the root and its entry) the first of its entry
    /// side traces whose checks took the frame, counted, with its entry:
    /// `true` then. When none did, and each refused the entry by the same
    /// word, notes that for the fast loop (`Proto::trace_head_refused`).
    #[cold]
    #[inline(never)]
    // the dispatch's state, as the dispatcher holds it
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trace_refused(
        &mut self,
        cl: Gc<LuaClosure>,
        entered: (&mut TArc<CompiledTrace>, &mut crate::jit::trace::TraceFn),
        pc: u32,
        base: u32,
        refusal: u32,
        reg_state: &mut LVec<i64>,
        entry_tags: &mut LVec<u8>,
    ) -> bool {
        let root = TArc::clone(entered.0);
        take_back_count(&root);
        let max_stack = cl.proto.max_stack as usize;
        // each taken out of the list, not held while it runs (it may add one)
        match first_taken(refusal, |k| {
            let es = root.entry_sides.list.borrow().get(k).cloned()?;
            Some((
                self.es_check(&es, base, max_stack, reg_state, entry_tags),
                es,
            ))
        }) {
            Ok((k, es)) => {
                let sides = &root.entry_sides;
                if sides.misses.get() < MAX_MISSES {
                    sides.last.set(k as u32);
                }
                self.enter_side(cl, entered, es);
                return true;
            }
            Err(true) => note_refusal(&cl.proto, pc, refusal),
            Err(false) => {}
        }
        if pc == 0 {
            self.count_refused_call(&cl.proto, &root);
        }
        false
    }

    /// Tries first, before the root `entered.0` (counted, with its entry
    /// `entered.1`) checks the frame at `base`, the entry side trace that
    /// took the last entry here, as it is likely to take this one: `true`,
    /// and `entered` the entry side trace, counted, with its entry, when it
    /// did; else the root is tried as usual, and first again after this.
    #[inline(never)]
    pub(super) fn trace_side_first(
        &mut self,
        cl: Gc<LuaClosure>,
        entered: (&mut TArc<CompiledTrace>, &mut crate::jit::trace::TraceFn),
        base: u32,
        reg_state: &mut LVec<i64>,
        entry_tags: &mut LVec<u8>,
    ) -> bool {
        let root = TArc::clone(entered.0);
        let sides = &root.entry_sides;
        let es = sides.list.borrow().get(sides.last.get() as usize).cloned();
        sides.last.set(NO_LAST);
        let Some(es) = es else { return false };
        let max_stack = cl.proto.max_stack as usize;
        let taken = self.es_check(&es, base, max_stack, reg_state, entry_tags) == entry::ENTERS;
        if !taken {
            sides.misses.set(sides.misses.get() + 1);
            return false;
        }
        take_back_count(&root);
        let k = sides
            .list
            .borrow()
            .iter()
            .position(|e| TArc::ptr_eq(e, &es));
        sides.last.set(k.map_or(NO_LAST, |k| k as u32));
        self.enter_side(cl, entered, es);
        true
    }

    /// What the entry side trace `es` makes of the frame at `base` of
    /// `max_stack` registers: [`entry::ENTERS`], the word it refuses the
    /// entry by, or [`NO_ROOM`] when the buffers cannot hold its window or
    /// the stack its inlined calls. Its window, not the root's, is the one
    /// checked: an entry side trace may inline a call its root does not.
    #[inline(always)]
    fn es_check(
        &mut self,
        es: &CompiledTrace,
        base: u32,
        max_stack: usize,
        reg_state: &mut LVec<i64>,
        entry_tags: &mut LVec<u8>,
    ) -> u32 {
        if !(words::frame_in(reg_state, es.window_size as usize, max_stack)
            && self.inlined_calls_fit(es.inline_depth, base, es.window_size))
        {
            return NO_ROOM;
        }
        self.trace_marshal_in(
            es,
            base as usize,
            &mut entry_tags[..max_stack],
            &mut reg_state[..max_stack],
        )
    }

    /// Puts in `entered` the entry side trace `es` of `cl`, counted, with
    /// its entry.
    #[inline(always)]
    fn enter_side(
        &mut self,
        cl: Gc<LuaClosure>,
        entered: (&mut TArc<CompiledTrace>, &mut crate::jit::trace::TraceFn),
        es: TArc<CompiledTrace>,
    ) {
        *entered.1 = self.trace_entry_counted(&es, cl.proto.call_hot_count.get());
        *entered.0 = es;
    }

    /// A call of `proto` refused by the trace `root` at its entry and each
    /// of its entry side traces: once as many came as make a call hot (and
    /// hot counting does not pause), the next call looks at recording an
    /// entry side trace (`Proto::trace_call_head_settled` is cleared for
    /// it). Calls taken pay nothing for it.
    fn count_refused_call(&mut self, proto: &crate::runtime::Proto, root: &CompiledTrace) {
        if self.jit.step.quiet.is_some() || !super::super::entry_side::may_have_more(root) {
            return;
        }
        let n = &root.entry_sides.call_refusals;
        n.set(n.get() + 1);
        if n.get() >= self.jit.call_hot_threshold {
            n.set(0);
            proto.trace_call_head_settled.set(false);
        }
    }
}

/// The word [`Vm::es_check`] gives an entry side trace whose window or
/// inlined calls do not fit: no refusal word is one, as the slot of one is
/// below a frame's size.
const NO_ROOM: u32 = entry::ENTERS - 1;

/// The first of the entry side traces `0..` that took the entry, by the
/// words `check` gives them in turn (with each, until it gives `None`):
/// `Ok` with its place and what `check` gave with its word. Else `Err`,
/// `true` when each refused the entry by `refusal`, the word the root
/// refused it by: the head may then be noted for the fast loop. One with no
/// room ([`NO_ROOM`]) a deeper stack would take, so the head is not noted.
fn first_taken<T>(
    refusal: u32,
    mut check: impl FnMut(usize) -> Option<(u32, T)>,
) -> Result<(usize, T), bool> {
    let mut same = true;
    let mut k = 0;
    while let Some((w, t)) = check(k) {
        if w == entry::ENTERS {
            return Ok((k, t));
        }
        same &= w == refusal;
        k += 1;
    }
    Err(same)
}

/// Takes back the count an entry refused by `root` made towards its move to
/// the optimizing tier, so that entries that never run do not have it
/// compile code for them.
fn take_back_count(root: &CompiledTrace) {
    if let Some(t) = &root.tier_up
        && !t.tried.get()
    {
        t.count.set(t.count.get().saturating_sub(1));
    }
}

/// Notes for the fast loop that the traces tried at head `pc` of `proto`
/// all refuse an entry by `refusal` (see `Proto::trace_head_refused`). A
/// head past those `trace_heads` lists is not watched for one by itself.
fn note_refusal(proto: &crate::runtime::Proto, pc: u32, refusal: u32) {
    if let Some(at) = proto.trace_heads.get().iter().position(|&h| h == pc) {
        proto.trace_head_refused[at].set(refusal);
    }
}

/// Clears what the fast loop notes for head `pc` of `proto`: a trace that
/// may take the entries refused there before was attached.
pub(in crate::vm::exec) fn forget_refusal(proto: &crate::runtime::Proto, pc: u32) {
    if let Some(at) = proto.trace_heads.get().iter().position(|&h| h == pc) {
        proto.trace_head_refused[at].set(crate::runtime::function::TRACE_HEAD_NOT_REFUSED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(refusal: u32, words: &[u32]) -> Result<usize, bool> {
        first_taken(refusal, |k| words.get(k).map(|&w| (w, ()))).map(|(k, ())| k)
    }

    #[test]
    fn the_first_that_takes_the_entry_is_entered() {
        assert_eq!(run(7, &[7, entry::ENTERS, entry::ENTERS]), Ok(1));
        assert_eq!(run(7, &[NO_ROOM, entry::ENTERS]), Ok(1));
    }

    #[test]
    fn a_head_is_noted_only_when_each_refuses_by_the_roots_word() {
        assert_eq!(run(7, &[]), Err(true));
        assert_eq!(run(7, &[7, 7]), Err(true));
        assert_eq!(run(7, &[7, 9]), Err(false));
    }

    #[test]
    fn one_with_no_room_for_its_inlined_calls_keeps_the_head_unnoted() {
        assert_eq!(run(7, &[NO_ROOM]), Err(false));
        assert_eq!(run(7, &[7, NO_ROOM, 7]), Err(false));
    }

    #[test]
    fn an_entry_side_trace_wider_than_its_frame_has_its_window_readied() {
        let vm = Vm::new(crate::version::LuaVersion::Lua54);
        let mut regs: LVec<i64> = LVec::new(vm.heap.mem());
        let (max_stack, window) = (4, 9);
        // a root's window: no wider than the frame
        assert!(words::frame_in(&mut regs, max_stack, max_stack));
        regs.iter_mut().for_each(|r| *r = -1);
        // the entry side trace inlined a call: the registers past the frame
        // are its, and start at zero
        assert!(words::frame_in(&mut regs, window, max_stack));
        assert!(regs.len() >= window + crate::jit::trace_types::frame::TRACE_FRAME_TAIL);
        assert!(regs[max_stack..window].iter().all(|&r| r == 0));
        assert!(regs[..max_stack].iter().all(|&r| r == -1));
    }

    #[test]
    fn an_entry_side_trace_whose_inlined_calls_overrun_the_stack_has_no_room() {
        let vm = Vm::new(crate::version::LuaVersion::Lua54);
        let last = vm.g.stack_last;
        // a root inlining nothing fits anywhere
        assert!(vm.inlined_calls_fit(0, last, 9));
        assert!(vm.inlined_calls_fit(1, 0, 9));
        // its entry side trace, one level deeper, at the stack's end
        assert!(!vm.inlined_calls_fit(1, last - 9, 9));
    }
}
