//! A compiled trace and the outcome of compiling one.

use super::*;

/// Native entry point for a compiled trace.
///
/// **ABI**:
///
/// ```text
/// fn(reg_state: *mut i64) -> i64
/// ```
///
/// - `reg_state` points to a caller-managed buffer of
///   `head_proto.max_stack` `i64` slots. The trace reads its live
///   inputs from this buffer at entry and writes back any modified
///   regs before returning. Each slot holds the raw 8-byte payload
///   of a Lua `Value`; the type tags live in [`CompiledTrace`]'s
///   tag arrays.
/// - Return value = continuation PC. A clean loop close (control
///   returns to the trace's `head_pc`) returns `head_pc as i64`;
///   side exits return the failing guard's PC (see
///   [`decode_exit_shape`] for the upper-bit encoding).
// SAFETY: `TraceFn` is the ABI of native code emitted by the Cranelift lowerer (see `jit_backend::trace`); callers guarantee the `*mut i64` points to a reg_state buffer of size `window_size` and survive the trace call.
pub type TraceFn = unsafe extern "C" fn(*mut i64) -> i64;

/// A trace compiled by the lowerer and ready to be dispatched into
/// at its head PC. Owned by `Proto.traces`; the underlying mmap is
/// kept alive by the `Vm.jit_handles` Vec for the Vm's lifetime,
/// just like the method JIT's compiled functions.
pub struct CompiledTrace {
    /// Pc the trace dispatches at (matches the recorder's `head_pc`).
    pub head_pc: u32,
    /// Native entry function (mmap'd machine code, valid for the Vm's
    /// lifetime as long as the backing handle is kept).
    pub entry: TraceFn,
    /// Number of ops in the source `TraceRecord`. Diagnostic only;
    /// tuning will gate re-record vs. recompile based on this.
    pub n_ops: u32,
    /// `true` iff the dispatcher can safely invoke this trace.
    /// False when the trace has ops the lowerer can't predict the
    /// exit type for (today: `Op::GetI` — the helper returns a
    /// raw payload that may be Int or Table or Float; without
    /// runtime tag info the dispatcher can't repack the slot).
    /// Non-dispatchable traces still compile and stay cached so
    /// a future dispatcher with richer marshalling can pick them
    /// up — they just don't run today.
    pub dispatchable: bool,
    /// Size of the reg_state buffer the dispatcher
    /// must allocate when calling `entry`. Today always equals
    /// `head_proto.max_stack` (the trace covers only the head
    /// frame). Inline emit pushes this past `max_stack`
    /// to fit additional inlined frames whose register windows sit
    /// at `offsets[i]..offsets[i] + max_stack` within the buffer.
    /// The dispatcher's marshal-in still only writes [0..max_stack)
    /// — depth>0 slots start initialized to zero, and the trace's
    /// own GetUpval / arith fills them as it runs.
    pub window_size: u32,
    /// Per-register exit tag of length `window_size`. Indexed by
    /// position within the trace's reg_state_buf. The dispatcher
    /// consults this to pack `reg_state[i]` back into a `Value`
    /// after the trace returns at the **clean tail** (head_pc or
    /// call-truncation pc). See [`ExitTag`] for the semantics.
    /// `Rc<[]>` so the dispatcher's per-dispatch lookup is a cheap
    /// refcount bump, not a Vec heap clone (fib_28 dispatches 1M×
    /// — clone cost dominates without this).
    pub exit_tags: TArc<[ExitTag]>,
    /// Classification of the global `exit_tags` for
    /// the dispatcher's restore-loop fast path. The dispatcher
    /// dispatches on this when `site_id == 0` AND
    /// `per_exit_tags.find(cont_pc)` misses (the common
    /// back-edge / clean-tail exit shape):
    /// - `AllUntouched` → skip the restore loop entirely (trace
    ///   touched no slots; vm.stack already holds the right
    ///   values from entry, possibly modified by spill helpers)
    /// - `AllInt`       → `vm.stack[base+i] = Value::Int(reg_state[i])`
    ///   per slot, no per-iter match
    /// - `Mixed`        → original match-arm loop
    pub global_tag_res_kind: TagResKind,
    /// Compile-time snapshot of `entry_tags` from the
    /// `TraceRecord`. The trace's IR + `current_kinds` propagation
    /// are specialised to these tags; if the runtime entry tags
    /// differ, the dispatcher must skip dispatch (fall back to
    /// interp) — otherwise the trace would treat e.g. a Str ptr
    /// slot as Int and produce garbage. `Rc<[]>` to match the
    /// other tag arrays' cheap-clone idiom.
    pub entry_tags: TArc<[u8]>,
    /// Per side-exit `exit_tags`. Each entry is
    /// `(continuation_pc, exit_tags)`; when the trace returns a PC
    /// matching an entry, the dispatcher uses that vector instead of
    /// the clean-tail `exit_tags`. This makes side-exits that fire
    /// **before** later writers (`GetUpval` is the today motivator)
    /// restore the affected slot as `Untouched` (carry entry tag)
    /// rather than pack with a tag the slot hasn't actually become.
    /// Empty when no side-exit needs a different vector than the
    /// clean tail (e.g. plain numeric loops with no GetUpval).
    pub per_exit_tags: TArc<[(u32, TArc<[ExitTag]>)]>,
    /// Per inline side-exit metadata, indexed by
    /// `site_idx`. Each entry carries the side-exit's `cont_pc`,
    /// the per-slot `exit_tags` snapshot (sized to `window_size` so
    /// every materialised frame's window is restored), and the
    /// frame-materialise `chain` to push.
    ///
    /// fib has SIBLING self-recursive Calls (pc7, pc11) and EVERY
    /// depth's cmp lands at the same `cont_pc` — keying the lookup
    /// by `cont_pc` alone would collapse all those
    /// distinct chains onto one entry. The trace IR encodes the
    /// firing site's `(site_idx + 1)` in the upper 32 bits of the
    /// returned i64 so the dispatcher disambiguates O(1).
    ///
    /// Empty when no cmp@d>0 fires in the trace. The IR pre-bakes
    /// each `chain`'s raw pointer (`Rc::as_ptr`) at compile time;
    /// the `Rc` clones in this field keep the slice alive for the
    /// trace's mmap lifetime (Proto.traces owns the CompiledTrace).
    pub per_exit_inline: TArc<[InlineSideExit]>,
    /// Per-exit hit counter (LuaJIT-study foundation for
    /// future side trace work). Length and layout:
    /// - `[0..per_exit_inline.len())`: parallel to per_exit_inline
    ///   (indexed by `site_id - 1` in the dispatcher).
    /// - `[per_exit_inline.len()..per_exit_inline.len()+per_exit_tags.len())`:
    ///   parallel to per_exit_tags (indexed by find-by-cont_pc order).
    /// - Last slot: global / clean-tail exit (when site_id == 0 AND
    ///   per_exit_tags.find misses).
    ///
    /// `Rc<[Cell<u32>]>` so the dispatcher can increment without a
    /// mutable borrow on the CompiledTrace. Vm's
    /// `trace_exit_hit_distribution()` aggregates this for probe use.
    pub exit_hit_counts: TArc<[TCellU32]>,
    /// Per-exit raw side-trace function pointer. Same
    /// length / layout as [`Self::exit_hit_counts`]. `null` means
    /// "no side trace compiled for this exit yet"; non-null means a
    /// child side trace's entry fn lives at this pointer.
    ///
    /// `Cell<*const u8>` (not Atomic) since the Vm is single-
    /// threaded. The pointer's stability is owned by
    /// the child side trace's `TraceHandle` in `TRACE_JIT_HANDLES`
    /// (thread-local Vec), which persists for the thread lifetime.
    ///
    /// **Send/Sync invariant**: `Cell<*const u8>` is not Sync, but
    /// `CompiledTrace` was never required to be Sync (it lives in
    /// `Proto.traces: RefCell<Vec<CompiledTrace>>` on the runtime
    /// path). Adding this field doesn't tighten that.
    pub exit_side_trace_ptrs: TArc<[TCellPtr]>,
    /// Per-`per_exit_tags`-entry side-trace cell.
    /// Same length as `per_exit_tags`; the IR at the corresponding
    /// `emit_store_back_and_return_pc` callsite (immediately after
    /// `per_exit_kinds.push`) bakes this cell's heap address. Same
    /// semantics as [`InlineSideExit::side_trace_ptr`] but with
    /// `kind = SIDE_SENT_KIND_TAG` and `local = tag_idx`.
    pub tags_side_trace_ptrs: TArc<[Box<TCellPtr>]>,
    /// Singleton cell shared by every GLOBAL-kind
    /// callsite (clean-tail return, Call truncation, ForLoop /
    /// TForLoop exits, generic err deopts, etc.). All such sites'
    /// IR bakes the same heap address; the close handler writes
    /// the child entry ptr here for `parent_exit_idx ==
    /// per_exit_inline.len() + per_exit_tags.len()` (the
    /// `exit_hit_counts` layout's last slot).
    pub global_side_trace_ptr: Box<TCellPtr>,
    /// When a child side trace compiles for any
    /// of this trace's hot exits, the close handler inserts
    /// `(child.head_pc, child_traces_idx)` here. The
    /// dispatcher uses this for an O(1) lookup of the side trace's
    /// own [`CompiledTrace`] when the sentinel bit on `raw_ret`
    /// flags a side-trace return — so
    /// [`decode_exit_shape`] can be called with the SIDE TRACE's
    /// `per_exit_inline` / `per_exit_tags` / `exit_tags` instead
    /// of the parent's.
    ///
    /// Value is an **index** into `head_proto.traces` (the same
    /// proto this `CompiledTrace` lives in — trace JIT only fires
    /// side traces from self-recursive parents today, so child +
    /// parent share `head_proto`). Storing an index instead of a
    /// raw pointer dodges the `Vec<CompiledTrace>` realloc-
    /// invalidation pitfall: `proto.traces.push` doesn't reorder,
    /// only appends, so an index assigned at compile time stays
    /// valid for the trace's lifetime.
    ///
    /// `RefCell<HashMap<u32, u32>>` because the close handler
    /// holds only `&CompiledTrace` (the parent's traces borrow is
    /// immutable while we're walking it to find the parent_ct).
    pub side_trace_cache: TRefLock<std::collections::HashMap<u32, u32>>,
    /// Fast-path short-circuit hint for the
    /// dispatcher's tentative-decode + cell-load + check path. Set
    /// to `true` by the close handler when ANY of this trace's
    /// `exit_side_trace_ptrs` cells gets wired (i.e., the first
    /// time a child side trace compiles + the shape gate
    /// passes). Stays `true` for the trace's lifetime — once any
    /// side trace exists, the dispatcher must perform the per-
    /// exit check on every dispatch.
    ///
    /// When `false`, the dispatcher skips the tentative decode +
    /// cell load + child lookup entirely, falling straight through
    /// to the cheap parent decode + writeback. Trims fib_10_x10k-
    /// class tight-trace workloads' per-dispatch overhead from the
    /// double-decode pattern to a single `Cell::get()`.
    ///
    /// `Cell<bool>` so the close handler can flip the flag through
    /// only an `&CompiledTrace` borrow (the parent's `traces`
    /// borrow is immutable while the close handler walks).
    pub has_any_side_wired: TCellBool,
    /// `true` iff this trace closes at a
    /// `TraceEnd::InlineAbort` (depth>0 op the lowerer can't
    /// continue past: depth past `MAX_INLINE_DEPTH`, non-self
    /// Call@d>0, ForLoop@d>0, TForLoop@d>0, or proto mismatch).
    /// Such traces compile but pin `dispatchable=false` —
    /// dispatching them would resume interp at a depth>0 PC
    /// without the matching CallFrames the trace inlined past
    /// (the frame mat helper can synthesise these but isn't wired
    /// up for InlineAbort exits). Vm's `trace_inline_abort_count`
    /// tallies these so future-tuning sees what bench cells
    /// would benefit from the frame-mat unlock.
    pub is_inline_abort_close: bool,
    /// If `dispatchable == false`, the static
    /// label of the emit-pass site that flipped it. Lets a probe
    /// distinguish among the six places trace.rs pins dispatch
    /// off (GetI / GetTable / GetUpval inference fail, TForCall
    /// slow-path, length gate, InlineAbort gate). `None` if the
    /// trace IS dispatchable, the first label otherwise.
    pub dispatch_off_reason: Option<&'static str>,
    /// Number of NewTable sites in this trace whose
    /// final `EscapeState` is `EscapeState::Sinkable` after
    /// the pre-emit demotion pass. Vm's
    /// `trace_sinkable_seen_count` tallies these for telemetry.
    pub sinkable_sites_seen: u32,
    /// Number of `AccumSite`s with `BufferState::Bufferable`
    /// detected by `detect_accumulators`. Count only; the sites are
    /// not yet used for buffered emit. Vm's `trace_accum_bufferable_seen_count`
    /// tallies these for probe visibility.
    pub accum_bufferable_seen: u32,
    /// Number of Sinkable sites this trace's emit
    /// actually allocated virt slot Variables for (i.e., took the
    /// no-heap-alloc path). Always `<= sinkable_sites_seen`. Bumps
    /// `Vm::trace_sunk_alloc_count` on compile success.
    pub sunk_alloc_seen: u32,
    /// Number of (site × cmp side-exit) pairs in this
    /// trace's IR that emit the materialise helper. Each pair is
    /// "this cmp's side-exit reconstructs site X's heap Table".
    /// Static count; the runtime number of helper calls depends
    /// on dispatch shape (which side-exits actually fire).
    pub materialize_emit_count: u32,
    /// Number of `Op::Closure` ops this trace's emit
    /// lowered to a `luna_jit_op_closure` helper call. Each
    /// closure-creating op replaces a `Heap::new_closure_inline`
    /// allocation, which dwarfs the dispatcher's marshal overhead;
    /// the length-gate skip below treats `closure_seen > 0` the
    /// same as `sunk_alloc_seen > 0` (don't gate short traces).
    pub closure_seen: u32,
    /// Sorted unique list of slot indices that ANY
    /// op in this trace's body WRITES (post `inline_depth` offset).
    /// Computed at compile via `compute_body_writes`; consumed
    /// by the smart side-trace gate at child compile to
    /// detect read-before-write live-in registers that would
    /// re-read the parent's stale exit value across the child's
    /// internal-loop iters.
    pub body_writes: Box<[u32]>,
    /// Down-recursion stitch link populated by
    /// the lowerer's `downrec_idx_opt` arm
    /// (`crates/luna-jit/src/jit_backend/trace.rs:7129+`) when a
    /// trace closes via `TraceEnd::DownRec`. Layout:
    /// `Some((trace_id_placeholder, target_head_pc))`. The lowerer
    /// emits a caller-pc guard at the close site that, on guard hit,
    /// returns the [`SIDE_SENT_DOWNREC_CODE`] sentinel — and on guard
    /// miss, falls back to the safe deopt-tail (store back caller
    /// window + return `head_pc` via GLOBAL sentinel).
    ///
    /// Field semantics:
    /// - `.0` = placeholder trace id. At compile time the trace
    ///   doesn't know its own index in `head_proto.traces` yet
    ///   (the index is assigned at the close handler's `traces.push`
    ///   site after this function returns). The lowerer writes `0`
    ///   here as a "this trace, self-stitch" sentinel; the dispatcher
    ///   interprets a non-`None` value with `.0 == 0` as "stitch
    ///   target = the trace currently dispatching" and uses
    ///   [`Self::head_pc`] for resolution. Mutual-recursion stitch
    ///   would need an explicit `head_proto.traces` index here.
    /// - `.1` = `target_head_pc`, copied from `record.head_pc` at
    ///   compile time. The stitch dispatcher tail-calls into the
    ///   target trace at this PC (which today = self, the trace
    ///   currently dispatching).
    ///
    /// `None` for every trace that doesn't close via `TraceEnd::
    /// DownRec`.
    pub downrec_link: Option<(u32, u32)>,
    // GC trace mcode lifetime invariant for the
    // multi-way stitch path. The lowerer's multi-way arm bakes
    // `dr_return_pc` + each retf's `caller_pc` into the IR as plain
    // `iconst(I64, _)` constants — none of these reach the runtime as
    // a pointer dereference. The stitch HIT path returns the DOWNREC
    // sentinel (a constant `u64`) and the deopt path stores back the
    // caller window + returns via the GLOBAL sentinel; neither path
    // dereferences any external trace's mcode. `downrec_link =
    // Some((0, head_pc))` is a `(u32, u32)` pair, `Copy`. No
    // `Box<Cell<*const u8>>` (the InlineSideExit / TAG / GLOBAL slot
    // shape) is involved.
    //
    // Consequence: this trace's mcode lifetime is governed solely by
    // its own `Rc<CompiledTrace>` strong-count (held by `proto.traces`
    // for as long as the proto lives). There is no cross-trace mcode
    // dependency, so the "child trace fn-ptr stale after parent
    // recompile" hazard doesn't apply — it would surface only if a
    // tail-call-into-target stitch wired `Rc<CompiledTrace>` /
    // `Weak<CompiledTrace>` into `parent_ct.side_trace_cache`.
    /// Number of distinct caller_pc candidates the
    /// lowerer baked into the multi-way guard at the
    /// `TraceEnd::DownRec` close. `0` for every trace that doesn't
    /// close via DownRec; `1` for single-CMP guards (the
    /// `dr_return_pc` alone, no additional retfs matched the close
    /// marker's `target_proto`); `>= 2` for multi-way
    /// guards that triggered the `dispatchable = true` lift.
    /// Capped at [`DOWNREC_MULTI_WAY_GUARD_MAX`].
    ///
    /// Read by the close handler in `crates/luna-core/src/vm/exec.rs`
    /// to bump `JitCounters.multi_way_guard_emitted`.
    pub downrec_multi_way_count: u8,
    /// Set when a quicker code generator compiled this trace and a better
    /// one can take over once it is hot.
    pub tier_up: Option<Box<TierUp>>,
    /// The functions of other prototypes the trace inlined. Its code checks
    /// a callee against these protos by address, so the collector keeps
    /// them alive while the trace lives (`Proto::trace` marks them).
    pub inlined_protos: Box<[Gc<Proto>]>,
}

/// A trace on its way to the optimizing tier.
pub struct TierUp {
    /// Loop iterations run in the trace's code plus entries: the code adds
    /// one per iteration through this cell (whose address it holds), the
    /// dispatcher one per entry.
    pub count: Box<TCellU32>,
    /// [`CompileOptions::tier_up_at`] the trace was compiled with.
    pub at: u32,
    /// The head function's `call_hot_count` when the trace was compiled.
    /// Once that function has been called again, the code is being reused
    /// rather than run once, and the trace moves after
    /// `at / TIER_UP_REUSED_DIVISOR` iterations and entries.
    pub calls_at: u32,
    /// The optimizing tier's entry once compiled (null before).
    pub optimized: TCellPtr,
    /// Set once the optimizing tier was asked, whatever it answered.
    pub tried: TCellBool,
    /// For a side trace: the parent's exit cells holding this trace's entry
    /// (addresses of [`TCellPtr`]s; null when not wired).
    pub parent_cells: [TCellPtr; 2],
    /// What the backend compiles the trace from; taken when it does.
    pub source: TRefLock<Option<Box<dyn std::any::Any + Send + Sync>>>,
}

impl CompiledTrace {
    /// The entry to call: the optimizing tier's once it exists.
    pub fn current_entry(&self) -> TraceFn {
        match &self.tier_up {
            Some(t) if !t.optimized.get().is_null() => {
                // SAFETY: `optimized` only ever holds an entry the backend
                // returned from `TraceCompiler::tier_up`, which has the
                // `TraceFn` ABI
                unsafe { std::mem::transmute::<*const u8, TraceFn>(t.optimized.get()) }
            }
            _ => self.entry,
        }
    }
}

impl std::fmt::Debug for CompiledTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledTrace")
            .field("head_pc", &self.head_pc)
            .field("n_ops", &self.n_ops)
            .field("dispatchable", &self.dispatchable)
            .field("exit_tags", &self.exit_tags)
            .field("entry", &"<fn>")
            .finish()
    }
}

/// Result of attempting to lower a closed [`TraceRecord`] to native
/// code. Most failure cases are recoverable — the recorder bumps the
/// head PC's failure count and refuses to re-record until the
/// threshold rolls over again.
#[derive(Debug)]
pub enum CompileOutcome {
    /// Trace compiled; the cached entry is ready for dispatch.
    Compiled,
    /// Some op in the trace falls outside the lowerer's whitelist (e.g. a
    /// metamethod-bearing operand, or a yet-unsupported opcode).
    /// The record is dropped; the head PC remembers the rejection.
    UnsupportedOp,
    /// Cranelift signaled an error during code emission. Should be
    /// rare in practice — usually a programmer error in the lowerer.
    BackendError,
}
