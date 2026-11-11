//! Entry side traces: a trace recorded from the head of a root trace for
//! the register types the root refuses on entry, attached to the root (not
//! to `Proto::traces`), which the dispatcher tries once the root refuses
//! an entry. Recorded from a back edge, as the root was, once the loop is
//! hot again with registers no trace at the head takes.

use super::*;
use crate::jit::send_compat::TArc;
use crate::jit::trace::{CompiledTrace, TraceRecord};
use crate::jit::trace_types::adopt::{entry_tags_admit, entry_tags_admit_with};
use crate::jit::trace_types::installed::entry_side::{ENTRY_SIDE_CAP, ENTRY_SIDE_HEAT};
use crate::vm::jit_state::JitCounters;

/// What a compile failure, or an attach refused, takes from a root's budget
/// for entry side traces; [`NEVER_ENTERED_COST`] for a recording that ends
/// another way. As the head budget's (`cache`).
const COMPILE_COST: u32 = 16;
const NEVER_ENTERED_COST: u32 = 3;

/// The trace at `pc` of `proto` that is no side trace, if any: the root
/// when it can be dispatched.
pub(in crate::vm::exec) fn head_trace(
    proto: &crate::runtime::Proto,
    pc: u32,
) -> Option<TArc<CompiledTrace>> {
    let traces = proto.traces.borrow();
    traces
        .iter()
        .find(|t| t.head_pc == pc && !t.is_side_trace())
        .cloned()
}

/// The traces of `proto`: those on its list, each root's entry side traces
/// after them.
pub(in crate::vm::exec) fn all_traces(proto: &crate::runtime::Proto) -> Vec<TArc<CompiledTrace>> {
    let traces = proto.traces.borrow();
    let mut all: Vec<_> = traces.iter().cloned().collect();
    for t in traces.iter() {
        all.extend(t.entry_sides.list.borrow().iter().cloned());
    }
    all
}

/// Whether entry side traces may still be recorded for `root`.
pub(in crate::vm::exec) fn may_have_more(root: &CompiledTrace) -> bool {
    // the budget goes to nothing once the list is full (`attach`)
    root.entry_sides.budget.get() > 0
        && root.dispatchable
        && root.downrec_link.is_none()
        && !root.call_count
}

/// Counts the times round its loop that `root` refused entries of the
/// kind the registers `regs` hold now (the tags of every register its
/// entry checks), as the loop is hot again (`threshold` times round since
/// it last was): whether that reached [`ENTRY_SIDE_HEAT`], the count then
/// starting again. Only the kind seen at the hot back edges is known: one
/// that changes in between starts the count again.
fn heated(root: &CompiledTrace, regs: &[crate::runtime::Value], threshold: u32) -> bool {
    let kind = entry_kind(root, regs);
    let s = &root.entry_sides;
    let round = threshold.saturating_add(1);
    if kind == s.heat_kind.get() {
        s.heat.set(s.heat.get().saturating_add(round));
    } else {
        s.heat_kind.set(kind);
        s.heat.set(round);
    }
    if s.heat.get() < ENTRY_SIDE_HEAT {
        return false;
    }
    s.heat.set(0);
    true
}

/// The kind of entry the registers `regs` make for `root`: a hash of the
/// place and tag of each register its entry checks.
pub(in crate::vm::exec) fn entry_kind(root: &CompiledTrace, regs: &[crate::runtime::Value]) -> u32 {
    let want = &root.plans.entry_tags;
    // FNV-1a
    let mut kind = 0x811c_9dc5_u32;
    for (i, v) in regs.iter().enumerate().take(want.len()) {
        if want[i] != crate::jit::trace::ENTRY_TAG_ANY {
            kind = (kind ^ (i as u32) << 8 ^ u32::from(v.unpack().0)).wrapping_mul(0x0100_0193);
        }
    }
    kind
}

/// Attaches `es` to `root`'s entry side traces: its place in the list, from
/// 1. A root holding as many as it may has no budget left for more.
fn attach(root: &CompiledTrace, es: TArc<CompiledTrace>) -> u8 {
    es.entry_sides.is_one.set(true);
    root.has_entry_side.set(true);
    let mut list = root.entry_sides.list.borrow_mut();
    list.push(es);
    if list.len() >= ENTRY_SIDE_CAP {
        root.entry_sides.budget.set(0);
    }
    list.len() as u8
}

/// Whether `root` or one of its entry side traces takes registers of tags
/// `now` (by the checks the dispatcher makes, but for registers past the
/// entry tags).
fn taken_by(root: &CompiledTrace, now: &[u8]) -> bool {
    taken_by_with(root, |i| now.get(i).copied())
}

/// [`taken_by`] for the registers `regs` hold: the hot back edge of a loop
/// whose trace takes them, the common case, makes nothing.
fn taken_by_regs(root: &CompiledTrace, regs: &[crate::runtime::Value]) -> bool {
    taken_by_with(root, |i| regs.get(i).map(|v| v.unpack().0))
}

fn taken_by_with(root: &CompiledTrace, now: impl Fn(usize) -> Option<u8> + Copy) -> bool {
    entry_tags_admit_with(&root.plans.entry_tags, now)
        || (root.entry_sides.list.borrow().iter())
            .any(|e| entry_tags_admit_with(&e.plans.entry_tags, now))
}

/// Takes `cost` from the budget of the root `rec` was recorded for, when
/// `rec` is an entry side trace's recording: `true` then, and the head's
/// own budget and the fail memory are left alone.
fn charge(rec: &TraceRecord, cost: u32) -> bool {
    if !rec.entry_side {
        return false;
    }
    if let Some(root) = head_trace(&rec.head_proto, rec.head_pc) {
        let b = &root.entry_sides.budget;
        b.set(b.get().saturating_sub(cost));
    }
    true
}

/// A recording that ended without a trace: counted against its head, or
/// against its root's entry side traces.
pub(in crate::vm::exec) fn note_recording_failure(counters: &mut JitCounters, rec: &TraceRecord) {
    if !charge(rec, COMPILE_COST) {
        note_trace_compile_failure(counters, rec.head_proto, rec.head_pc);
    }
}

/// [`note_recording_failure`] for a recording the trace could never be
/// entered from.
pub(in crate::vm::exec) fn note_recording_never_entered(
    counters: &mut JitCounters,
    rec: &TraceRecord,
) {
    if !charge(rec, NEVER_ENTERED_COST) {
        note_trace_never_entered(counters, rec.head_proto, rec.head_pc, &rec.entry_tags);
    }
}

/// A call-triggered recording for an entry side trace dropped for covering
/// too little of its function: counted against its root.
pub(in crate::vm::exec) fn note_partial_entry_side(rec: &TraceRecord) {
    charge(rec, NEVER_ENTERED_COST);
}

/// Whether the closed entry side trace recording `rec` would add nothing:
/// its root is gone, can have no more, or has one that takes its entry.
pub(in crate::vm::exec) fn entry_side_there_already(rec: &TraceRecord) -> bool {
    head_trace(&rec.head_proto, rec.head_pc)
        .is_none_or(|root| !may_have_more(&root) || taken_by(&root, &rec.entry_tags))
}

impl Vm {
    /// The loop head `target` of `cl`'s prototype (frame `base`) is hot
    /// again, and `root` is the trace there that is no side trace: starts
    /// recording an entry side trace of it, when it may have one and
    /// neither it nor one of its entry side traces takes the registers now.
    /// `true` when a recording started.
    pub(in crate::vm::exec) fn trace_start_entry_side(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        target: u32,
        tfor: Option<(u32, u32)>,
        root: &CompiledTrace,
    ) -> bool {
        if !may_have_more(root) {
            return false;
        }
        let max_stack = cl.proto.max_stack as usize;
        let Some(regs) = self.stack.get(base as usize..base as usize + max_stack) else {
            return false;
        };
        if taken_by_regs(root, regs) || !heated(root, regs, self.jit.trace_hot_threshold) {
            return false;
        }
        // one another Vm compiled for these registers first
        if let Some(attached) = self.trace_adopt_entry_side(cl.proto, target, base, false, root) {
            return attached;
        }
        self.trace_start_at_loop(cl, base, target, tfor);
        if let Some(rec) = self.jit.active_trace.as_mut() {
            rec.entry_side = true;
        }
        true
    }

    /// A call of `cl` (its frame `frame_idx`, at `base_us`) when calls the
    /// trace `root` at its entry refused came to be hot: starts recording an
    /// entry side trace of it there, as [`Self::trace_start_entry_side`]
    /// does at a loop head.
    pub(in crate::vm::exec) fn trace_start_call_entry_side(
        &mut self,
        cl: Gc<LuaClosure>,
        frame_idx: usize,
        base_us: usize,
        root: &CompiledTrace,
    ) {
        if !may_have_more(root) {
            return;
        }
        let max_stack = cl.proto.max_stack as usize;
        let Some(regs) = self.stack.get(base_us..base_us + max_stack) else {
            return;
        };
        let now: Vec<u8> = regs.iter().map(|v| v.unpack().0).collect();
        if taken_by(root, &now)
            || self
                .trace_adopt_entry_side(cl.proto, 0, base_us as u32, true, root)
                .is_some()
        {
            return;
        }
        let mut rec = TraceRecord::start(cl.proto, 0, now, true);
        rec.settings = self.jit.recording_settings();
        rec.entry_side = true;
        self.jit.active_trace = Some(Box::new(rec));
        self.jit.recording_frame_base = frame_idx;
    }

    /// Takes up, as an entry side trace of `root` (at `head_pc` of `proto`,
    /// frame base `base`), a trace another Vm compiled for the registers now
    /// (to it, a root as any other), with its side traces. `None` when there
    /// is none; `Some(false)` when the one taken up may not be entered from
    /// `root` (counted against it, as a compile failure would be).
    pub(in crate::vm::exec) fn trace_adopt_entry_side(
        &mut self,
        proto: Gc<crate::runtime::Proto>,
        head_pc: u32,
        base: u32,
        call_triggered: bool,
        root: &CompiledTrace,
    ) -> Option<bool> {
        let (mut adopted, now) =
            self.trace_adopt_request(proto, head_pc, base as usize, None, call_triggered, false)?;
        if adopted.is_empty() {
            return None;
        }
        let rest = adopted.split_off(1);
        let es = adopted.pop()?;
        let root_id = crate::jit::trace::ParentId::of(proto, root)?;
        let fits = es.side_parent.is_none()
            && es.trace.dispatchable
            && es.trace.downrec_link.is_none()
            && !es.trace.call_count
            && entry_tags_admit(&es.trace.plans.entry_tags, &now)
            && self.trace_room(es.trace.window_size as usize).is_ok();
        if !fits {
            let b = &root.entry_sides.budget;
            b.set(b.get().saturating_sub(COMPILE_COST));
            return Some(false);
        }
        self.tally_compiled_trace(&es.trace);
        let side = attach(root, es.trace);
        super::dispatch::forget_refusal(&proto, head_pc);
        keep_inlined(proto, es.inlined);
        self.heap.barrier_back(proto);
        self.jit.counters.adopted += 1;
        let named = vec![(es.image, crate::jit::trace::ParentId { side, ..root_id })];
        self.trace_install_adopted(proto, rest, named);
        Some(true)
    }

    /// Attaches the AOT binary's precompiled `ct` to the root trace parked
    /// at its head of `proto` before, as an entry side trace: `false` when
    /// there is none, it may have no more, `ct` cannot be dispatched, or
    /// one it holds was compiled for the same entry tags.
    pub(in crate::vm::exec) fn trace_attach_aot_entry_side(
        &mut self,
        proto: Gc<crate::runtime::Proto>,
        ct: CompiledTrace,
    ) -> bool {
        let Some(root) = head_trace(&proto, ct.head_pc) else {
            return false;
        };
        let same = |t: &CompiledTrace| t.plans.entry_tags == ct.plans.entry_tags;
        let fits = may_have_more(&root)
            && ct.dispatchable
            && ct.downrec_link.is_none()
            && !ct.call_count
            && !same(&root)
            && !root.entry_sides.list.borrow().iter().any(|e| same(e));
        if !fits {
            return false;
        }
        let pc = ct.head_pc;
        attach(&root, TArc::new(ct));
        super::dispatch::forget_refusal(&proto, pc);
        true
    }

    /// Attaches `ct`, compiled from the entry side trace recording `rec`,
    /// to its root, when the dispatcher may enter it from there: it can be
    /// dispatched, takes the entry it was recorded from, and there is room
    /// to run it. Else counts a failure against the root.
    pub(in crate::vm::exec) fn trace_attach_entry_side(
        &mut self,
        ct: CompiledTrace,
        rec: &TraceRecord,
    ) {
        let proto = rec.head_proto;
        let Some(root) = head_trace(&proto, rec.head_pc) else {
            return;
        };
        let fits = ct.dispatchable
            && ct.downrec_link.is_none()
            && !ct.call_count
            && entry_tags_admit(&ct.plans.entry_tags, &rec.entry_tags)
            && self.trace_room(ct.window_size as usize).is_ok();
        if !fits {
            charge(rec, COMPILE_COST);
            self.jit.counters.compile_failed += 1;
            return;
        }
        self.tally_compiled_trace(&ct);
        attach(&root, TArc::new(ct));
        super::dispatch::forget_refusal(&proto, rec.head_pc);
        keep_inlined(
            proto,
            rec.ops
                .iter()
                .filter(|op| op.inline_depth > 0)
                .map(|op| op.proto),
        );
        self.heap.barrier_back(proto);
        self.jit.counters.compiled += 1;
    }
}
