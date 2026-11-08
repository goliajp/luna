//! `JitState` sidecar: JIT-specific Vm state factored out of the
//! [`crate::vm::Vm`] struct, one field hop away from the interpreter's
//! (`self.jit.active_trace`), so the two are kept apart at the field level.
//!
//! `JitState` is always present on a Vm — even an embedder that never runs
//! JIT'd code holds an inert one whose `chunk_compiler` / `trace_compiler`
//! are [`crate::jit::NullJitBackend`]. The dispatcher reads
//! `self.jit.chunk_compiler` on every JIT entry; `Option<JitState>` would
//! put an `unwrap()` on that path for no benefit.
//!
//! luna's other crates (the compiled code's helpers write
//! `pending_err` from `extern "C"` functions) reach it through
//! `luna_core::__private`, outside the public API.

use crate::runtime::mem::{LBox, LVec, MemRef};
use crate::vm::error::LuaError;

mod counters;
mod run_words;
pub use counters::JitCounters;
pub(crate) use run_words::BLOCKED_UNMADE;
pub use run_words::{RunWords, RunWordsOffsets};

/// JIT-specific Vm state. See module docs.
pub struct JitState {
    /// Master JIT switch. Off until a real backend is installed
    /// ([`crate::vm::Vm::install_jit_backend`]), which turns it on unless
    /// the embedder already chose a value with `Vm::set_jit_enabled`.
    /// An armed instruction budget or memory cap keeps compiled code out
    /// whatever this says (`Vm::limited`).
    pub enabled: bool,
    /// `enabled` with no limit armed: what the call paths test before
    /// entering compiled code. Written by `set_jit_flag` / `sync_limited`.
    pub(crate) gate: bool,

    /// Trace JIT subswitch. Same default and install rule as
    /// [`Self::enabled`].
    pub trace_enabled: bool,

    /// The embedder set [`Self::enabled`] / [`Self::trace_enabled`]
    /// explicitly; installing a backend then leaves that flag alone.
    pub(crate) enabled_chosen: bool,
    pub(crate) trace_enabled_chosen: bool,

    /// Back-edge visits before a loop is recorded as a trace, and calls
    /// before a function is. [`crate::jit::trace::TRACE_HOT_THRESHOLD`]
    /// and [`crate::jit::trace::CALL_HOT_THRESHOLD`] by default; tests
    /// lower them so that short programs exercise the trace JIT.
    pub trace_hot_threshold: u32,
    /// See [`Self::trace_hot_threshold`].
    pub call_hot_threshold: u32,
    /// The code generator traces are compiled with; `LUNA_TRACE_TIER`
    /// (`auto`, `baseline` or `optimizing`) sets the default.
    pub trace_tier: crate::jit::trace::TraceTier,
    /// [`crate::jit::trace::CompileOptions::tier_up_at`] for this Vm's
    /// traces.
    pub tier_up_at: u32,
    /// Ask the trace compiler for traces other Vms compiled before
    /// recording one (see [`crate::vm::Vm::enable_trace_sharing`]).
    pub(crate) share_traces: bool,

    /// Back-edge counts per loop head, indexed by a hash of the head's
    /// pc and its function's first line (LuaJIT's `hotcount`): each loop
    /// of a function gets hot on its own. Two heads sharing a slot make
    /// each other hot sooner, which changes where recording starts; the
    /// hash uses no address so that this is the same on every run.
    pub(crate) loop_hot: Box<[u32; LOOP_HOT_SLOTS]>,

    /// Opt-in flag for the self-link cycle catch. Default `false`:
    /// the catch has a known correctness problem, so it ships disabled.
    pub self_link_enabled: bool,

    /// Whether the recorder captures a table-field inline cache
    /// snapshot (see [`crate::jit::trace_types::field_ic_enabled`]).
    /// Starts from `LUNA_JIT_FIELD_IC`; the lowering only emits the
    /// cache for a trace whose record carries the snapshot.
    pub(crate) field_ic_enabled: bool,

    /// The trace being recorded, if any.
    pub active_trace: Option<Box<crate::jit::trace::TraceRecord>>,

    /// Index into `Vm.frames` of the Lua frame that the
    /// recorder started in.
    pub recording_frame_base: usize,

    /// Running max of `inline_depth` observed on
    /// any `RecordedOp` pushed by the recorder.
    pub max_depth_seen: u8,

    /// Diagnostic counters; see [`JitCounters`].
    pub counters: JitCounters,

    /// Set by a helper of compiled code that cannot go on (a metatable on
    /// the table, no memory): the dispatcher runs the call in the
    /// interpreter instead. `None` outside compiled code. Written through
    /// [`JitState::set_pending_err`], which keeps [`JitState::words`].
    pending_err: Option<LuaError>,

    /// The trace dispatcher's `reg_state`, kept between entries.
    pub reg_state_buf: LVec<i64>,
    /// The links compiled code took since the dispatches running began, as
    /// `[parent, typed exit, linked trace]` (the traces by their index in
    /// the frame's prototype's list); room for `LINK_DEPTH` is kept.
    pub(crate) link_trail: LVec<[u32; 3]>,
    /// Where the innermost dispatch's links start in `link_trail`.
    pub(crate) link_base: usize,

    /// Roots: values compiled code holds only in registers while it calls a
    /// helper that can collect. The address of the innermost list (0 none):
    /// in the trace's frame, `n`, the list it was linked over, `n` pairs of
    /// tag and payload words. Linked for the call, unlinked after.
    pub ssa_roots: usize,

    /// The string accumulator buffers: the first `str_bufs_free` kept for
    /// reuse, the others held by the traces running now.
    pub(crate) str_bufs: LVec<LBox<LVec<u8>>>,
    pub(crate) str_bufs_free: u32,
    /// The records of PUC's objects each running trace stands for
    /// (`TraceGhosts`), by the depth of nested trace runs, and that depth.
    pub(crate) ghosts: Vec<crate::vm::exec::TraceGhosts>,
    pub(crate) ghost_depth: usize,
    /// Companion buffer for `entry_tags` (one u8 per register at entry).
    pub entry_tags_buf: LVec<u8>,

    /// The trace running now, for a collection step it runs in place.
    pub(crate) step: crate::vm::exec::TraceStep,
    /// Closure-compile backend the dispatcher routes through
    /// (`Vm::install_jit_backend`; default [`crate::jit::NullJitBackend`]).
    pub chunk_compiler: Box<dyn crate::jit::IntChunkCompiler>,

    /// Trace-JIT backend.
    pub trace_compiler: Box<dyn crate::jit::TraceCompiler>,

    /// Downrec admits left before the dispatcher refuses one (a trace that
    /// stitches back to itself could return the DOWNREC sentinel forever)
    /// and force-deopts through [`Self::suppress_downrec_admit_once`];
    /// reset to [`JitState::STITCH_DEPTH_DEFAULT`] at each natural deopt
    /// and when that flag fires.
    pub stitch_depth_remaining: u32,

    /// One-shot suppression flag for the dispatcher's trace admit. Set
    /// when a trace hands control back at its own `head_pc` without
    /// having run the op there: the
    /// dispatcher when it force-deopts a downrec entry (guard miss OR
    /// cycle-budget exhausted), and a trace side exit taken before the
    /// head op (through `luna_jit_suppress_trace_admit`). The NEXT
    /// interpreter loop iteration skips the admit and lets interp run
    /// the op at `head_pc`, advancing `pc` past `head_pc` and breaking
    /// the otherwise-infinite admit loop. Consumed (cleared) the first
    /// time the dispatcher reads it.
    pub suppress_downrec_admit_once: bool,

    /// Per-`Vm` JIT storage holder.
    /// Default is [`crate::jit::NullJitStorage`]; the `luna_jit`
    /// crate's `install_default_jit` swaps in a
    /// `CraneliftJitStorage` carrying the cache + compiled-handle
    /// collections. Accessed via downcast
    /// from the `CraneliftBackend` trait impls.
    pub storage: Box<dyn crate::jit::JitStorage>,

    /// An error raised by a call that compiled code handed to the
    /// interpreter (a self-recursive call made with too little native
    /// stack left): unlike `pending_err` it is the call's outcome, raised
    /// where the compiled code was entered, not a reason to run it again.
    pub pending_raise: Option<LuaError>,
    /// compiled code left after an allocation with a collection step due:
    /// the interpreter checks for it as after the op that allocated, at
    /// this pc (`u32::MAX`: the one before the pc it resumes at). Written
    /// through [`JitState::set_gc_exit`], which keeps [`JitState::words`].
    gc_exit: Option<u32>,
    /// What compiled code reads of this Vm (see [`RunWords`]).
    pub(crate) words: Box<RunWords>,
    /// `g.frames_native` outside the compiled call running now: its levels
    /// are counted above it (`Vm::jit_self_level`)
    pub levels_base: u32,
    /// the slot of the compiled call's level 0 (its base)
    pub levels_slot: u32,
    /// where the compiled levels handed to the interpreter left their
    /// frames on the stack
    pub(crate) handoff: crate::vm::exec::HandoffMarks,
    /// heads that failed to compile, by their function's content; boxed
    /// on the first failure, since most Vms never have one
    pub(crate) fail_memory: Option<Box<crate::vm::exec::FailMemory>>,
}

impl JitState {
    /// Default per-dispatch stitch-back depth. The downrec admit
    /// is gated by a multi-way CMP-chain that is a real runtime
    /// guard (not constant-folded), so the only
    /// way a downrec trace HITs is when `saved_pc` from the parent
    /// frame matches one of the recorded `caller_pc` candidates;
    /// each natural admit corresponds to ONE Lua call chain pop, so
    /// the budget can safely grow to cover ~all consecutive HITs expected
    /// in a hot loop without infinite-loop risk. `32` lets 31 HITs
    /// accumulate before a forced-deopt resets the budget; fib(3)'s hot
    /// loop shows 1 HIT every 5 admits, so `32` covers ~32 outer iters.
    pub const STITCH_DEPTH_DEFAULT: u32 = 32;
}

impl JitState {
    /// [`crate::jit::trace::TraceRecord::settings`] for a recording now.
    pub(crate) fn recording_settings(&self) -> u8 {
        u8::from(self.field_ic_enabled) | (u8::from(self.self_link_enabled) << 1)
    }
}

/// Slots in [`JitState::loop_hot`].
pub(crate) const LOOP_HOT_SLOTS: usize = 256;

impl JitState {
    /// Count a back-edge to the loop head at pc `head` of `proto`; `true`
    /// once that head has been crossed more than
    /// [`Self::trace_hot_threshold`] times, which starts its count over.
    #[inline]
    pub(crate) fn loop_hot_tick(&mut self, proto: &crate::runtime::Proto, head: u32) -> bool {
        let key = proto
            .line_defined
            .wrapping_mul(0x9e37_79b1)
            .wrapping_add(head);
        let slot = &mut self.loop_hot[key as usize & (LOOP_HOT_SLOTS - 1)];
        if *slot >= self.trace_hot_threshold {
            *slot = 0;
            true
        } else {
            *slot += 1;
            false
        }
    }

    /// Build an inert `JitState` whose backends are
    /// [`crate::jit::NullJitBackend`], with `enabled` and
    /// `trace_enabled` off: nothing could compile, so the interpreter
    /// skips the hot counters, the recorder and the trace lookup.
    /// `Vm::new_inner` calls this; the `luna` crate's
    /// `Vm::new_minimal_with_jit` then swaps the backends to
    /// `CraneliftBackend` via `Vm::install_jit_backend`, which turns
    /// both flags on. Its buffers allocate through `mem`.
    pub fn with_null_backend(mem: MemRef) -> JitState {
        JitState {
            enabled: false,
            gate: false,
            trace_enabled: false,
            enabled_chosen: false,
            trace_enabled_chosen: false,
            trace_hot_threshold: crate::jit::trace::TRACE_HOT_THRESHOLD,
            call_hot_threshold: crate::jit::trace::CALL_HOT_THRESHOLD,
            trace_tier: default_trace_tier(),
            tier_up_at: crate::jit::trace::TIER_UP_THRESHOLD,
            share_traces: false,
            loop_hot: Box::new([0; LOOP_HOT_SLOTS]),
            self_link_enabled: false,
            field_ic_enabled: crate::jit::trace_types::field_ic_enabled(),
            active_trace: None,
            recording_frame_base: 0,
            max_depth_seen: 0,
            counters: JitCounters::default(),
            pending_err: None,
            pending_raise: None,
            gc_exit: None,
            words: Box::new(RunWords::new()),
            levels_base: 0,
            levels_slot: 0,
            handoff: Default::default(),
            fail_memory: None,
            reg_state_buf: LVec::new(mem),
            link_trail: LVec::new(mem),
            link_base: 0,
            ssa_roots: 0,
            str_bufs: LVec::new(mem),
            str_bufs_free: 0,
            entry_tags_buf: LVec::new(mem),
            ghosts: Vec::new(),
            ghost_depth: 0,
            step: Default::default(),
            chunk_compiler: Box::new(crate::jit::NullJitBackend),
            trace_compiler: Box::new(crate::jit::NullJitBackend),
            stitch_depth_remaining: JitState::STITCH_DEPTH_DEFAULT,
            suppress_downrec_admit_once: false,
            storage: Box::new(crate::jit::NullJitStorage),
        }
    }
}

fn default_trace_tier() -> crate::jit::trace::TraceTier {
    use crate::jit::trace::TraceTier;
    static TIER: std::sync::OnceLock<TraceTier> = std::sync::OnceLock::new();
    *TIER.get_or_init(|| match std::env::var("LUNA_TRACE_TIER").as_deref() {
        Ok("baseline") => TraceTier::Baseline,
        Ok("optimizing") => TraceTier::Optimizing,
        _ => TraceTier::Auto,
    })
}
