//! Installing AOT traces, and the per-proto trace cache and compile
//! failure blacklist.

use super::*;
use crate::vm::jit_state::JitCounters;

// The deploy-side resolver in `luna-runtime-helpers` walks the binary's trace-meta section after
// `vm.load`, resolves each entry's `(proto_hash, head_pc, fn_ptr)` triple against the loaded
// chunk's proto tree, and pushes a `CompiledTrace` onto the matching Proto's `traces` Vec via
// [`crate::__private::install_aot_trace`]. The existing trace-dispatch loop then fires the AOT mcode
// without further plumbing, the same code path the runtime JIT uses.

impl Vm {
    /// Park an AOT binary's precompiled `trace` on `proto.traces`, where
    /// the dispatcher enters it at its `head_pc` as it would a trace the
    /// JIT compiled. `__private::install_aot_trace` states what the trace
    /// must be. `false`, the trace not parked, when there is no room for
    /// what running it needs (see [`Vm::trace_room`]).
    pub(crate) fn install_aot_trace(
        &mut self,
        proto: crate::runtime::Gc<crate::runtime::function::Proto>,
        trace: crate::jit::trace::CompiledTrace,
    ) -> bool {
        if self.trace_room(trace.window_size as usize).is_err() {
            self.jit.counters.install_no_room += 1;
            return false;
        }
        cache_trace(proto, trace);
        true
    }

    /// Walk the proto tree
    /// reachable from `root` and return `(proto, stable_hash)` pairs
    /// for every Proto found. Used by the deploy-side resolver to
    /// match AOT-emitted `proto_hash` keys against the freshly
    /// `undump`'d chunk's protos.
    ///
    /// The walk is BFS over `Proto.protos`. Same-Proto deduplication
    /// is done via `Gc::as_ptr` identity — a Proto re-referenced from
    /// multiple nested closures (rare; the cache field would catch
    /// the closure-side dedup, not the Proto side) is reported once.
    ///
    /// # Why on `&Vm` and not a free fn
    ///
    /// Keeps the AOT install API discoverable on the Vm surface —
    /// `vm.collect_proto_hashes(root)` reads naturally next to
    /// `vm.install_aot_trace(proto, trace)`. Doesn't actually touch
    /// any Vm field, so `&self` (read-only) is enough.
    pub fn collect_proto_hashes(
        &self,
        root: crate::runtime::Gc<crate::runtime::function::Proto>,
    ) -> Vec<(
        crate::runtime::Gc<crate::runtime::function::Proto>,
        [u8; 16],
    )> {
        let _ = self;
        let mut out = Vec::new();
        let mut seen: std::collections::HashSet<*const crate::runtime::function::Proto> =
            std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<
            crate::runtime::Gc<crate::runtime::function::Proto>,
        > = std::collections::VecDeque::new();
        queue.push_back(root);
        while let Some(p) = queue.pop_front() {
            let key = p.as_ptr() as *const _;
            if !seen.insert(key) {
                continue;
            }
            out.push((p, p.stable_hash()));
            for &child in p.protos.iter() {
                queue.push_back(child);
            }
        }
        out
    }
}

/// Recordings of one trace head that may fail to compile, or overflow the
/// recorder, before the head is no longer recorded (LuaJIT likewise
/// blacklists a trace start after repeated failures). A few tries, since a
/// later recording can see different register kinds or take a shorter path.
pub(in crate::vm::exec) const MAX_TRACE_COMPILE_FAILURES: u8 = 3;

/// Recordings of a head that could never be entered (a register the trace
/// reads held a boolean on entry), before the head is no longer recorded.
/// More than other failures: the head is not recorded again while those
/// registers still hold such values, so each of these follows a change.
pub(in crate::vm::exec) const MAX_TRACE_NEVER_ENTERED: u8 = 16;

/// A head's failures are counted in a budget of this many units: a compile
/// failure takes `BUDGET / MAX_TRACE_COMPILE_FAILURES`, a recording that
/// could never be entered `BUDGET / MAX_TRACE_NEVER_ENTERED`.
const BUDGET: u8 = 48;

pub(in crate::vm::exec) fn note_trace_compile_failure(
    counters: &mut JitCounters,
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
) {
    note_head_failure(
        counters,
        proto,
        head_pc,
        BUDGET / MAX_TRACE_COMPILE_FAILURES,
        None,
    );
}

/// `entry_tags`: the recording's tags on entry.
pub(in crate::vm::exec) fn note_trace_never_entered(
    counters: &mut JitCounters,
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
    entry_tags: &[u8],
) {
    let stuck = (0..entry_tags.len() as u16)
        .filter(|&r| !crate::jit::trace::entry_tag_enterable(entry_tags[r as usize]))
        .collect();
    note_head_failure(
        counters,
        proto,
        head_pc,
        BUDGET / MAX_TRACE_NEVER_ENTERED,
        Some(stuck),
    );
}

fn note_head_failure(
    counters: &mut JitCounters,
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
    cost: u8,
    stuck: Option<Vec<u16>>,
) {
    use crate::runtime::function::HeadFailures;
    let mut failures = proto.trace_compile_failures.borrow_mut();
    let i = match failures.iter().position(|f| f.head_pc == head_pc) {
        Some(i) => i,
        None => {
            failures.push(HeadFailures {
                head_pc,
                n: 0,
                stuck: Vec::new(),
            });
            failures.len() - 1
        }
    };
    let f = &mut failures[i];
    let n = f.n.saturating_add(cost);
    counters.blacklisted += u64::from(f.n < BUDGET && n >= BUDGET);
    f.n = n;
    f.stuck = stuck.unwrap_or_default();
    if head_pc == 0 && f.n >= BUDGET {
        proto.trace_call_head_settled.set(true);
    }
}

/// Whether recording at `head_pc` of `proto`, with the frame's registers
/// `regs`, would again give a trace that can never be entered.
pub(in crate::vm::exec) fn trace_head_stuck(
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
    regs: &[Value],
) -> bool {
    proto
        .trace_compile_failures
        .borrow()
        .iter()
        .find(|f| f.head_pc == head_pc)
        .is_some_and(|f| {
            !f.stuck.is_empty()
                && f.stuck.iter().all(|&r| {
                    regs.get(r as usize)
                        .is_some_and(|v| !crate::jit::trace::entry_tag_enterable(v.unpack().0))
                })
        })
}

/// Park `ct` on `proto.traces`, keeping `has_dispatchable_trace` in step
/// with the dispatcher's admit test.
pub(in crate::vm::exec) fn cache_trace(
    proto: Gc<crate::runtime::function::Proto>,
    ct: crate::jit::trace::CompiledTrace,
) -> TArc<crate::jit::trace::CompiledTrace> {
    if ct.dispatchable || ct.downrec_link.is_some() {
        proto.has_dispatchable_trace.set(true);
        use crate::runtime::function::{TRACE_HEADS_CAP, TRACE_HEADS_MANY, TRACE_HEADS_NONE};
        let mut heads = proto.trace_heads.get();
        if heads[0] != TRACE_HEADS_MANY && !heads.contains(&ct.head_pc) {
            match heads.iter_mut().find(|h| **h == TRACE_HEADS_NONE) {
                Some(h) => *h = ct.head_pc,
                None => heads = [TRACE_HEADS_MANY; TRACE_HEADS_CAP],
            }
        }
        proto.trace_heads.set(heads);
    }
    if ct.head_pc == 0 {
        proto.trace_call_head_settled.set(true);
    }
    let ct = TArc::new(ct);
    // side cells link traces of one prototype by plain pointers, relying on
    // this list never losing a trace: removing one must clear those links
    // first
    proto.traces.borrow_mut().push(ct.clone());
    ct
}

/// [`cache_trace`] for a trace compiled from `record`, keeping alive the
/// prototypes of the other functions it inlined.
pub(in crate::vm::exec) fn cache_compiled_trace(
    proto: Gc<crate::runtime::function::Proto>,
    ct: crate::jit::trace::CompiledTrace,
    record: &crate::jit::trace::TraceRecord,
) -> TArc<crate::jit::trace::CompiledTrace> {
    let ct = cache_trace(proto, ct);
    keep_inlined(
        proto,
        record
            .ops
            .iter()
            .filter(|op| op.inline_depth > 0)
            .map(|op| op.proto),
    );
    ct
}

/// Keeps the prototypes in `inlined` alive while `proto` lives: a trace
/// cached on `proto` checks callees against them by address.
pub(in crate::vm::exec) fn keep_inlined(
    proto: Gc<crate::runtime::function::Proto>,
    inlined: impl IntoIterator<Item = Gc<crate::runtime::function::Proto>>,
) {
    let mut kept = proto.inlined_protos.borrow_mut();
    for p in inlined {
        if !p.ptr_eq(proto) && !kept.iter().any(|k| k.ptr_eq(p)) {
            kept.push(p);
        }
    }
}

/// Gives up recording at `head_pc` of `proto`.
pub(in crate::vm::exec) fn give_up_head(
    counters: &mut JitCounters,
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
) {
    note_head_failure(counters, proto, head_pc, BUDGET, None);
}

pub(in crate::vm::exec) fn trace_head_abandoned(
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
) -> bool {
    proto
        .trace_compile_failures
        .borrow()
        .iter()
        .any(|f| f.head_pc == head_pc && f.n >= BUDGET)
}
