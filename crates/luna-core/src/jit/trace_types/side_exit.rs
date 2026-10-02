//! Side exits: inline side-exit records, hot-exit counting and sentinels.

use super::*;

/// Per inline cmp@d>0 side-exit record. See
/// [`CompiledTrace::per_exit_inline`] for the shape rationale.
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
    /// Slot-by-slot `ExitTag` snapshot at the side-exit moment.
    /// Length = `window_size` — covers caller + every inlined
    /// frame's register window.
    pub exit_tags: TArc<[ExitTag]>,
    /// Frames to push onto `vm.frames` (outermost = depth 1 first,
    /// innermost = depth `len()` last). The innermost frame's `pc`
    /// is overwritten to the side-exit PC at compile time so the
    /// helper stays PC-agnostic.
    pub chain: TArc<[FrameMaterializeInfo]>,
    /// Raw `*const u8` (entry fn pointer of a child
    /// side trace) for THIS inline cmp@d>0 side-exit. The IR at the
    /// `emit_store_back_and_return_site` call site loads this cell
    /// BEFORE the encoded-return path: non-null → store-back +
    /// `call_indirect` into the child + OR sentinel(INLINE, site_idx)
    /// into bits 56..=63 of the child's return + return; null → run
    /// the existing encoded-return path.
    ///
    /// `Box<Cell<*const u8>>` (not embedded Cell) so the cell's HEAP
    /// address is stable for the IR's `iconst`-baked load. Moving
    /// the Box (e.g. into `Rc<[]>` via `.collect`) doesn't move the
    /// cell. Single-threaded Vm so `Cell` is sound.
    pub side_trace_ptr: Box<TCellPtr>,
}

/// Hot side-exit detection threshold. Exits whose hit
/// count crosses this value are reported by `Vm::hot_exit_iter` as
/// side-trace candidates. LuaJIT 2.1's default is 10, but short
/// workloads (binary_trees_d4_x200 = 200 outer iters, each calling
/// make/itemcheck a small handful of times) don't reach 10 hot
/// hits before the run ends, so the threshold is kept low to give
/// short workloads a chance to wire side traces.
pub const HOTEXIT_THRESHOLD: u32 = 2;

/// Sentinel kind tags for side-trace returns.
/// When a parent trace's IR detects a wired child side-trace cell
/// non-null at a side-exit and tail-calls into the child, it OR's
/// a 7-bit sentinel into the upper bits of the child's return value
/// (bit 63 = side-trace marker, bits 56..=62 = `encode_side_sentinel
/// (kind, local)`). The dispatcher reads the marker to know it must
/// re-decode the body using the SIDE TRACE's shape inputs, not the
/// parent's. The kind is informational (debug + close-handler routes
/// the right cell write); `local` disambiguates among multiple wired
/// cells of the same kind (e.g. several inline cmp@d>0 sites).
pub const SIDE_SENT_KIND_INLINE: u8 = 1;
/// Sentinel kind for tag-cell side-traces (typed-register exits).
pub const SIDE_SENT_KIND_TAG: u8 = 2;
/// Sentinel kind for global-cell side-traces (env-table exits).
pub const SIDE_SENT_KIND_GLOBAL: u8 = 3;
/// Sentinel kind for down-recursion stitch
/// returns. Emitted at the `TraceEnd::DownRec` close arm in
/// `crates/luna-jit/src/jit_backend/trace.rs` `downrec_idx_opt`
/// branch when the caller-pc guard hits (saved `[base-8]` matches
/// the recorded `target_proto`'s expected return PC) so the
/// dispatcher knows to walk the parent trace's RetfRecord
/// chain to materialise the inlined frames and tail-call into the
/// stitched child trace rather than falling back to interp at
/// `head_pc`.
pub const SIDE_SENT_KIND_DOWNREC: u8 = 4;

/// Encoded sentinel value reserved for
/// [`SIDE_SENT_KIND_DOWNREC`]. Picked as `0x10` (= 16) which sits
/// in the (kind=0, local=0..=31) slice unused by existing kinds
/// 1..=3 (those occupy encoded ranges 32..=127). DOWNREC has no
/// `local` (only 1 stitch slot per trace today), so the value is
/// a single constant rather than a function of `local`. Out-of-band
/// vs the regular `((kind & 0x3) << 5) | (local & 0x1F)` layout —
/// keeps existing kinds' encoding (and TAG local cap of 32) intact
/// without widening the kind bits.
pub const SIDE_SENT_DOWNREC_CODE: u32 = 0x10;

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

/// Encode a `(kind, local)` pair into a 7-bit
/// sentinel code that fits in `raw_ret`'s bits 56..=62. Layout for
/// kinds 1..=3: upper 2 bits = kind, lower 5 bits = local index.
/// A local index `>= 32` is truncated; the close handler caps
/// tag-cell allocation at 32 to avoid sentinel collisions. The
/// dispatcher uses the full 7-bit value as the key into the
/// parent's `side_trace_cache`.
///
/// Kind 4 ([`SIDE_SENT_KIND_DOWNREC`])
/// is encoded out-of-band as [`SIDE_SENT_DOWNREC_CODE`] (= 0x10).
/// DOWNREC has only one slot per trace (the stitch target lives on
/// `CompiledTrace.downrec_link`, not in the side_trace_cache), so
/// its `local` is asserted to be 0.
pub fn encode_side_sentinel(kind: u8, local: u32) -> u32 {
    debug_assert!(
        (1..=4).contains(&kind),
        "kind must be SIDE_SENT_KIND_* (1..=4)"
    );
    if kind == SIDE_SENT_KIND_DOWNREC {
        debug_assert!(
            local == 0,
            "SIDE_SENT_KIND_DOWNREC requires local == 0 (single stitch slot per trace)"
        );
        return SIDE_SENT_DOWNREC_CODE;
    }
    ((kind as u32 & 0x3) << 5) | (local & 0x1F)
}

/// True iff the dispatcher's decoded
/// `sentinel_code` (`(raw_ret >> 56) & 0x7F` at
/// the dispatcher) marks a down-recursion stitch return. Shared so
/// the lowerer, the dispatcher and any diagnostic probe use one
/// definition.
#[inline]
pub fn is_downrec_sentinel(sentinel_code: u32) -> bool {
    sentinel_code == SIDE_SENT_DOWNREC_CODE
}

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
/// All-`Copy` fields with no padding inside each field — 12 bytes
/// per entry on amd64.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FrameMaterializeInfo {
    /// Stack offset (relative to the trace head's `frame.base`) of
    /// the callee's first register slot. The new frame's `base` is
    /// `head_frame.base + base_offset`; its `func_slot` is one below.
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
    /// from this call (encoded as `Op::Call`'s C - 1). The
    /// pre-emit pass bails if any inlined Call has nresults != 1
    /// (Op::Return1 copy-back assumes one value).
    pub nresults: i32,
}
