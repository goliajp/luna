//! Copying a frame's registers into a trace's buffers before it runs, by the
//! trace's entry plan (`EntryPlan`).

use super::*;
use crate::jit::trace::{ENTRY_TAG_ANY, PLAIN_ENTRY_TAGS};
use crate::jit::trace_types::installed::entry_plan::{EntryPlan, EntrySlot};

impl Vm {
    /// Copy the frame's registers, `entry_tags.len()` of them from `base_us`,
    /// into the trace's entry buffers `entry_tags` and `reg_state` (as
    /// long). [`ENTERS`], or, when a register the trace checks has a tag
    /// other than the one `ct` was compiled for, or one that cannot be
    /// passed as a raw payload, that register and its tag as `slot << 8 |
    /// tag`, which alone refuse the entry (one word: the dispatcher tests it
    /// as it would a `bool`). A trace with no side trace wired copies only the
    /// registers it checks. The others are held on the stack (see the lowering's
    /// `StackHeld`), and only a side trace run from one of its exits reads
    /// them, or their entry tags, from the buffers. Out of line, so that the
    /// loop has the registers to itself.
    #[inline(never)]
    pub(super) fn trace_marshal_in(
        &self,
        ct: &crate::jit::trace::CompiledTrace,
        base_us: usize,
        entry_tags: &mut [u8],
        reg_state: &mut [i64],
    ) -> u32 {
        let l = entry_tags.len();
        let frame = &self.stack[base_us..base_us + l];
        let (regs, tags) = (&mut reg_state[..l], &mut entry_tags[..l]);
        let plan = &ct.entry_plan;
        if let Err(k) = checked_slots(plan.checked(), frame, regs, tags)
            && let Err((slot, tag)) = odd_slots(&plan.checked()[k..], frame, regs, tags)
        {
            return refusal(slot, tag);
        }
        if ct.has_any_side_wired.get() {
            unchecked_in(plan, frame, regs, tags);
        }
        let n = plan.len();
        if n >= l {
            return ENTERS;
        }
        match past_slots(&frame[n..], &mut regs[n..], &mut tags[n..]) {
            Ok(()) => ENTERS,
            Err((i, tag)) => refusal((n + i) as u16, tag),
        }
    }
}

/// The registers of a trace with a side trace wired that its entry plan
/// `plan` leaves unchecked, copied in as they are: not read before the
/// trace writes them, so any value enters, and an exit that has not
/// written one leaves it as it is (a side trace may read it). The gaps
/// between the checked registers, which the plan lists in slot order.
#[inline(never)]
fn unchecked_in(plan: &EntryPlan, frame: &[Value], regs: &mut [i64], tags: &mut [u8]) {
    let n = plan.len().min(frame.len());
    let mut copy = |from: usize, to: usize| {
        for s in from..to {
            regs[s] = unpack_word(frame[s]).1;
            tags[s] = ENTRY_TAG_ANY;
        }
    };
    let mut next = 0;
    for e in plan.checked() {
        let s = usize::from(e.slot);
        copy(next, s.min(n));
        next = s + 1;
    }
    copy(next.min(n), n);
}

/// What [`Vm::trace_marshal_in`] returns for an entry its checks pass: no
/// refusal is this word, as a register's slot fits 16 bits.
pub(super) const ENTERS: u32 = u32::MAX;

/// A refusal at register `slot`, holding a value of tag `tag`, as
/// [`Vm::trace_marshal_in`] returns it.
#[inline]
fn refusal(slot: u16, tag: u8) -> u32 {
    u32::from(slot) << 8 | u32::from(tag)
}

/// The checked registers of an entry plan whose tag is the plain match of
/// the one they were compiled for; the index of the first that is not.
/// The trace's IR is specialised to the compile-time entry tags: on
/// another, body ops would misread the raw bits (a Str pointer as an Int
/// payload). A boolean, the one entry type two tags enter, is left to
/// [`odd_slots`], so that the common case costs one comparison and one bit
/// test, and the loop calls nothing.
#[inline(always)]
fn checked_slots(
    plan: &[EntrySlot],
    frame: &[Value],
    regs: &mut [i64],
    tags: &mut [u8],
) -> Result<(), usize> {
    let l = frame.len();
    let (regs, tags) = (&mut regs[..l], &mut tags[..l]);
    for k in 0..plan.len() {
        let EntrySlot { slot, tag: want } = plan[k];
        let s = slot as usize;
        if s >= l {
            continue;
        }
        let (tag, payload) = unpack_word(frame[s]);
        if tag != want || PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
            return Err(k);
        }
        regs[s] = payload;
        tags[s] = tag;
    }
    Ok(())
}

/// [`checked_slots`] from the register it stopped at on, each register
/// that is not a plain match passed through [`bool_entry`]. The slot and
/// tag of the first that cannot enter.
#[cold]
#[inline(never)]
fn odd_slots(
    plan: &[EntrySlot],
    frame: &[Value],
    regs: &mut [i64],
    tags: &mut [u8],
) -> Result<(), (u16, u8)> {
    for &EntrySlot { slot, tag: want } in plan {
        let s = slot as usize;
        if s >= frame.len() {
            continue;
        }
        let (tag, payload) = unpack_word(frame[s]);
        regs[s] = payload;
        if tag != want || PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
            match bool_entry(want, tag) {
                Some(p) => regs[s] = p,
                None => return Err((slot, tag)),
            }
        }
        tags[s] = tag;
    }
    Ok(())
}

/// The frame's registers past the trace's entry tags, each checked like one
/// read: against its own tag. The index and tag of the first that cannot
/// enter.
#[cold]
#[inline(never)]
fn past_slots(frame: &[Value], regs: &mut [i64], tags: &mut [u8]) -> Result<(), (usize, u8)> {
    for (i, (v, (r, t))) in frame
        .iter()
        .zip(regs.iter_mut().zip(tags.iter_mut()))
        .enumerate()
    {
        let (tag, payload) = unpack_word(*v);
        *r = payload;
        if PLAIN_ENTRY_TAGS >> tag & 1 == 0 {
            match bool_entry(tag, tag) {
                Some(p) => *r = p,
                None => return Err((i, tag)),
            }
        }
        *t = tag;
    }
    Ok(())
}

/// `v`'s tag and its payload word as a register holds it.
#[inline(always)]
fn unpack_word(v: Value) -> (u8, i64) {
    let (tag, payload) = v.unpack();
    // SAFETY: every field of `RawVal` is one plain 8-byte word, and
    // `unpack` returns it fully initialised
    (tag, unsafe { payload.zero as i64 })
}

/// The payload a register of tag `tag` enters a trace with whose entry tag
/// for it is `want`, when `tag` is not the plain match of `want`: a trace
/// compiled for a boolean takes either value, as payload 0 or 1. `None`:
/// the register cannot enter.
#[cold]
#[inline(never)]
fn bool_entry(want: u8, tag: u8) -> Option<i64> {
    use crate::runtime::value::raw;
    (want == raw::FALSE && (tag == raw::FALSE || tag == raw::TRUE)).then(|| i64::from(tag - want))
}
