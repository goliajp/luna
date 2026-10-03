//! Installing AOT traces, and the per-proto trace cache and compile
//! failure blacklist.

use super::*;

// The deploy-side resolver in `luna-runtime-helpers` walks the binary's
// trace-meta section after `vm.load`, resolves each entry's
// `(proto_hash, head_pc, fn_ptr)` triple against the loaded chunk's
// proto tree, and pushes a `CompiledTrace` onto the matching Proto's
// `traces` Vec via [`Vm::install_aot_trace`] below. The existing
// trace-dispatch loop (`cl.proto.traces.borrow().iter()
// .find(|t| t.head_pc == pc && t.dispatchable)`) then fires the AOT
// mcode without further plumbing — same code path the runtime JIT
// uses.

impl Vm {
    /// Install a precompiled
    /// `CompiledTrace` onto `proto.traces` so the interp dispatcher
    /// fires it at the trace's `head_pc`. This is the runtime install
    /// API the deploy-side `luna-runtime-helpers` resolver calls once
    /// per AOT-emitted trace meta entry, after looking up `proto` by
    /// stable hash (see `crate::runtime::function::Proto::stable_hash`).
    ///
    /// # What this does
    ///
    /// Pushes `trace` onto `proto.traces` via the existing `RefCell`.
    /// The trace's `entry` fn ptr must already point at runnable
    /// machine code (the AOT linker resolved the symbol at link time;
    /// the deploy resolver passes the address verbatim).
    ///
    /// # What this does NOT do
    ///
    /// - **No deduplication.** Calling twice with the same `head_pc`
    ///   pushes two entries; the dispatcher's `find` will pick the
    ///   first match. The deploy resolver is responsible for not
    ///   double-installing.
    /// - **No invalidation of the runtime JIT cache.** If the runtime
    ///   JIT later records + compiles a trace for the same
    ///   `(proto, head_pc)`, both coexist on `proto.traces` and the
    ///   dispatcher's `find` picks whichever appears first. AOT
    ///   traces install before any runtime recording is possible
    ///   (resolver runs before `vm.load` returns its first closure),
    ///   so AOT traces win the race for the same site.
    /// - **No coverage gating.** AOT traces are trusted by
    ///   construction — they were validated at compile time. Setting
    ///   `dispatchable: false` on the input would silently disable
    ///   dispatch; the caller controls that flag.
    ///
    /// # Safety / soundness
    ///
    /// `trace.entry` is an `unsafe extern "C" fn` (mmap'd or linked
    /// machine code). Soundness contract:
    ///
    /// - The fn pointer must remain valid for the `Vm`'s lifetime.
    ///   In the AOT-binary deploy shape this is trivially satisfied —
    ///   the fn lives in the binary's `.text`.
    /// - `trace.entry_tags` / `exit_tags` / `window_size` must match
    ///   what the trace's IR actually compiled against; the dispatcher
    ///   uses them to marshal `reg_state` in and out without further
    ///   validation. A mismatch corrupts vm.stack.
    ///
    /// The AOT pipeline (`luna-aot`) is responsible for ensuring these
    /// invariants hold; this fn is a plain push — no validation that
    /// would slow the dispatcher's hot path either.
    pub fn install_aot_trace(
        &mut self,
        proto: crate::runtime::Gc<crate::runtime::function::Proto>,
        trace: crate::jit::trace::CompiledTrace,
    ) {
        let _ = self; // resolver passes &mut Vm for symmetry with future
        // pending-install + hash-walk variants; nothing on `self` to
        // mutate today because the install target lives on the Proto.
        cache_trace(proto, trace);
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
pub(super) const MAX_TRACE_COMPILE_FAILURES: u8 = 3;

/// Recordings of a head that could never be entered (a register the trace
/// reads held a boolean on entry), before the head is no longer recorded.
/// More than other failures: the head is not recorded again while those
/// registers still hold such values, so each of these follows a change.
pub(super) const MAX_TRACE_NEVER_ENTERED: u8 = 16;

/// A head's failures are counted in a budget of this many units: a compile
/// failure takes `BUDGET / MAX_TRACE_COMPILE_FAILURES`, a recording that
/// could never be entered `BUDGET / MAX_TRACE_NEVER_ENTERED`.
const BUDGET: u8 = 48;

pub(super) fn note_trace_compile_failure(proto: Gc<crate::runtime::function::Proto>, head_pc: u32) {
    note_head_failure(proto, head_pc, BUDGET / MAX_TRACE_COMPILE_FAILURES, None);
}

/// `entry_tags`: the recording's tags on entry.
pub(super) fn note_trace_never_entered(
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
    entry_tags: &[u8],
) {
    let stuck = (0..entry_tags.len() as u16)
        .filter(|&r| !crate::jit::trace::entry_tag_enterable(entry_tags[r as usize]))
        .collect();
    note_head_failure(
        proto,
        head_pc,
        BUDGET / MAX_TRACE_NEVER_ENTERED,
        Some(stuck),
    );
}

fn note_head_failure(
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
    f.n = f.n.saturating_add(cost);
    f.stuck = stuck.unwrap_or_default();
    if head_pc == 0 && f.n >= BUDGET {
        proto.trace_call_head_settled.set(true);
    }
}

/// Whether recording at `head_pc` of `proto`, with the frame's registers
/// `regs`, would again give a trace that can never be entered.
pub(super) fn trace_head_stuck(
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
pub(super) fn cache_trace(
    proto: Gc<crate::runtime::function::Proto>,
    ct: crate::jit::trace::CompiledTrace,
) {
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
    proto.traces.borrow_mut().push(TArc::new(ct));
}

/// [`cache_trace`] for a trace compiled from `record`, keeping alive the
/// prototypes of the other functions it inlined.
pub(super) fn cache_compiled_trace(
    proto: Gc<crate::runtime::function::Proto>,
    ct: crate::jit::trace::CompiledTrace,
    record: &crate::jit::trace::TraceRecord,
) {
    cache_trace(proto, ct);
    keep_inlined(
        proto,
        record
            .ops
            .iter()
            .filter(|op| op.inline_depth > 0)
            .map(|op| op.proto),
    );
}

/// Keeps the prototypes in `inlined` alive while `proto` lives: a trace
/// cached on `proto` checks callees against them by address.
pub(super) fn keep_inlined(
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

pub(super) fn trace_head_abandoned(
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
) -> bool {
    proto
        .trace_compile_failures
        .borrow()
        .iter()
        .any(|f| f.head_pc == head_pc && f.n >= BUDGET)
}
