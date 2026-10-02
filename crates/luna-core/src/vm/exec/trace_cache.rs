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

pub(super) fn note_trace_compile_failure(proto: Gc<crate::runtime::function::Proto>, head_pc: u32) {
    let mut failures = proto.trace_compile_failures.borrow_mut();
    let n = match failures.iter_mut().find(|(pc, _)| *pc == head_pc) {
        Some((_, n)) => {
            *n = n.saturating_add(1);
            *n
        }
        None => {
            failures.push((head_pc, 1));
            1
        }
    };
    if head_pc == 0 && n >= MAX_TRACE_COMPILE_FAILURES {
        proto.trace_call_head_settled.set(true);
    }
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

pub(super) fn trace_head_abandoned(
    proto: Gc<crate::runtime::function::Proto>,
    head_pc: u32,
) -> bool {
    proto
        .trace_compile_failures
        .borrow()
        .iter()
        .any(|&(pc, n)| pc == head_pc && n >= MAX_TRACE_COMPILE_FAILURES)
}
