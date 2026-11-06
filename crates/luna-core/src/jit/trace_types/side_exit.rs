//! Side exits: inline side-exit records, hot-exit counting and sentinels.

use super::*;

/// Per inline cmp@d>0 side-exit record. See
/// [`CompiledTraceParts::per_exit_inline`] for the shape rationale.
#[derive(Clone, Debug)]
pub struct InlineSideExit {
    /// PC the interpreter resumes at after the side-exit fires.
    /// Mirrors the innermost frame's `pc` in `chain`.
    pub cont_pc: u32,
    /// PC to write on the trace head frame when the side-exit
    /// fires — the depth-0 frame's resume point after ITS own Call
    /// that entered depth 1. Without this update, the trace head
    /// frame's pc stays at `head_pc` (where the dispatcher entered);
    /// once the inlined chain pops, interp resumes the trace head
    /// at pc=0 and immediately self-Calls again → infinite dispatch
    /// loop. Captured at emit time as the outermost `Op::Call`'s
    /// `pc + 1` from the live `call_chain`.
    pub head_resume_pc: u32,
    /// Frames to push onto `vm.frames` (outermost = depth 1 first,
    /// innermost = depth `len()` last). The innermost frame's `pc`
    /// is overwritten to the side-exit PC at compile time so the
    /// helper stays PC-agnostic.
    pub chain: TArc<[FrameMaterializeInfo]>,
}

/// Hot side-exit detection threshold. Exits whose hit
/// count crosses this value are reported by `Vm::hot_exit_iter` as
/// side-trace candidates. LuaJIT 2.1's default is 10, but short
/// workloads (binary_trees_d4_x200 = 200 outer iters, each calling
/// make/itemcheck a small handful of times) don't reach 10 hot
/// hits before the run ends, so the threshold is kept low to give
/// short workloads a chance to wire side traces.
pub const HOTEXIT_THRESHOLD: u32 = 2;

/// Upper bound on the multi-way caller-pc guard
/// chain emitted at the `TraceEnd::DownRec` close in the lowerer
/// (`crates/luna-jit/src/jit_backend/trace.rs` `downrec_idx_opt` arm).
/// The lowerer dedupes `record.retfs` by `caller_pc` (filtered to
/// retfs whose `proto` matches the close marker's `target_proto`) and
/// emits up to this many `icmp(Equal, saved_pc, iconst(candidate_pc))
/// + brif(stitch_blk, next_blk)` chain entries before falling through
/// to `deopt_blk`. A single CMP measured a 90%
/// miss-rate on fib(3) hot-loop; the typical fib body shape captures
/// 2 distinct caller_pcs (one per call site `pc+1`), so a cap of 4
/// covers the fib pattern with headroom for slightly deeper closes
/// without growing IR proportional to retfs.len(). When the candidate
/// set reaches >= 2 entries, the lowerer also sets `dispatchable =
/// true` so the primary dispatcher arm hits the trace without going
/// through the `downrec_link.is_some()` fallback admit clause.
pub const DOWNREC_MULTI_WAY_GUARD_MAX: usize = 4;

/// Env-gated probe switch. `LUNA_V2C_PROBE=1` (any
/// non-empty value) turns on the side-trace dispatch probes (IR
/// side-entry, dispatcher decode, frame.pc set). Off by default
/// so production runs pay no overhead — even the IR-emitted probe
/// call is conditional on the probe helper itself short-circuiting
/// when the OnceLock resolves to `false`.
static V2C_PROBE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// True iff the side-trace dispatch probes are enabled via
/// `LUNA_V2C_PROBE=1`. Diagnostic-only; production builds keep this
/// off so the IR-emitted probe call short-circuits cheaply.
pub fn v2c_probe_enabled() -> bool {
    *V2C_PROBE_ON.get_or_init(|| {
        std::env::var("LUNA_V2C_PROBE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
    })
}

/// One hot side-exit candidate surfaced by
/// `Vm::hot_exit_iter`. The walker fills this from one
/// [`CompiledTrace`]'s `exit_hit_counts` slot whose value passed
/// [`HOTEXIT_THRESHOLD`].
///
/// `head_proto` + `head_pc` identify the *parent* trace; `exit_idx`
/// indexes into the parent's `exit_hit_counts` (same layout the
/// dispatcher uses to bump). `cont_pc` is where the interpreter
/// resumed after the side-exit; this is the side trace's natural
/// entry PC. `exit_tags` is the compile-time slot-shape snapshot the
/// side trace would inherit as its entry tags.
#[derive(Clone, Debug)]
pub struct HotExitInfo {
    /// The trace head's Proto. `head_proto.traces` owns the parent
    /// [`CompiledTrace`]; combined with `head_pc` it uniquely
    /// identifies which trace this exit belongs to.
    pub head_proto: Gc<Proto>,
    /// PC of the parent trace's head (== the entry the dispatcher
    /// looks up under `cl.proto.traces`).
    pub head_pc: u32,
    /// Index into the parent's `exit_hit_counts`. Layout:
    /// - `[0..per_exit_inline.len())`: inline cmp@d>0 side-exits
    /// - `[per_exit_inline.len()..per_exit_inline.len() + per_exit_tags.len())`:
    ///   per-cont_pc side-exits (GetUpval-style)
    /// - last slot: global clean-tail / back-edge fallback
    pub exit_idx: usize,
    /// Saturating count from `exit_hit_counts[exit_idx]` at the
    /// moment of the walk. Always `>= HOTEXIT_THRESHOLD`.
    pub hits: u32,
    /// PC the interpreter resumed at after this side-exit fired.
    /// Inline side-exits read from `InlineSideExit.cont_pc`;
    /// per_exit_tags entries from their `(cont_pc, _)` pair; the
    /// global slot reports `head_pc` (the clean-tail back-edge
    /// returns to the trace's head, where dispatch can re-enter).
    pub cont_pc: u32,
    /// Slot-shape snapshot at the exit moment, reused as the side
    /// trace's entry_tags. Inline side-exits cover the full
    /// `window_size` (caller + inlined frames); per_exit_tags
    /// entries cover only the caller's `max_stack`; the global
    /// slot exposes the clean-tail `exit_tags` (caller window only).
    pub exit_tags: TArc<[ExitTag]>,
}

/// One Lua frame to push when a depth>0 side-exit
/// fires. Constructed at trace compile time from the recorded
/// `Op::Call` chain's `A` field (caller's `R[A]` = function slot) and
/// the inlined callee's `c` field (`nresults`). `pc` is the address
/// the helper writes onto the freshly-pushed frame so the interp
/// resumes at the right offset inside the callee body.
///
/// `repr(C)` because the trace's IR loads the array via raw pointer
/// arithmetic; Rust's default `repr` doesn't guarantee field order.
/// Four 32-bit fields, no padding: 16 bytes per entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FrameMaterializeInfo {
    /// Stack offset (relative to the trace head's `frame.base`) of
    /// the callee's first register slot. The new frame's `base` is
    /// `head_frame.base + base_offset`; its `func_slot` is
    /// `n_varargs + 1` below.
    pub base_offset: u32,
    /// PC to write on the freshly-pushed frame. For inner frames
    /// (not the innermost) this is the caller's Call.pc + 1 so the
    /// interp resumes after the Call instruction. For the innermost
    /// frame (the one the side-exit fires inside) the dispatcher
    /// overrides this with the actual side-exit PC — keeps the
    /// helper PC-agnostic (the helper doesn't know which frame is
    /// innermost).
    pub pc: u32,
    /// PUC `nresults`: how many return values the caller expects
    /// from this call (`Op::Call`'s C - 1; -1 for all of them).
    pub nresults: i32,
    /// The extra arguments of a vararg callee, which sit between the
    /// function slot and `base_offset` (as `push_frame` leaves them).
    pub n_varargs: u32,
}

impl CompiledTrace {
    /// The register tags exit `exit_idx` (laid out as `exit_hit_counts`)
    /// leaves, over the whole window from the trace's head frame.
    pub(crate) fn exit_tags_of(&self, exit_idx: usize) -> &[ExitTag] {
        self.plans.exit_tags(exit_idx)
    }

    /// Where the frame exit `exit_idx` resumes in starts, in registers from
    /// the trace's head frame: the base of the innermost frame an exit
    /// inside an inlined function rebuilds, 0 for an exit in the head frame.
    /// A side trace started at that exit runs on the registers from there.
    pub(crate) fn exit_frame_offset(&self, exit_idx: usize) -> usize {
        self.per_exit_inline
            .get(exit_idx)
            .and_then(|e| e.chain.last())
            .map_or(0, |f| f.base_offset as usize)
    }

    /// Whether a side trace is wired to exit `exit_idx` (laid out as
    /// `exit_hit_counts`).
    pub(crate) fn has_side_trace(&self, exit_idx: usize) -> bool {
        self.side_children
            .borrow()
            .get(exit_idx)
            .is_some_and(Option::is_some)
    }
}
