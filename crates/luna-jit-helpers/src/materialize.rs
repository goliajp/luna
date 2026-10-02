//! Frame materialization for inline side exits.

use crate::{current_jit_closure, current_jit_vm};

/// Runtime fire counter for the inline-chain reloc path. Every call to
/// [`luna_jit_trace_materialize_frames`] from trace mcode (JIT-baked
/// OR AOT slot-loaded) increments this counter. In an AOT-
/// only run (no in-process JIT compilation of traces that carry
/// inline cmp@d>0 side-exits) any non-zero value is direct evidence
/// that the chain reloc path actually fires at runtime — the
/// resolver-side probe (`aot_inline_chains_resolved`) only confirms
/// the slot was populated, not that any AOT mcode dispatch ever
/// loaded it.
pub static TRACE_MATERIALIZE_FRAMES_FIRES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Reader for [`TRACE_MATERIALIZE_FRAMES_FIRES`]. Relaxed load is fine
/// — the counter is diagnostic, not a synchronisation point.
pub fn trace_materialize_frames_fires() -> u64 {
    TRACE_MATERIALIZE_FRAMES_FIRES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Frame materialization helper.
///
/// Walks `metas[0..n]` and pushes one
/// `CallFrame::Lua` per entry onto `vm.frames` so the interp can
/// resume at a depth>0 continuation PC after the trace side-exits.
/// Returns `0` on success, non-zero to force the dispatcher into
/// the deopt path. The lowerer emits the call site from cmp@d>0
/// side-exit blocks.
///
/// Invariants the caller (lowerer) enforces at compile time:
/// - All inlined frames are the same `LuaClosure` (self-recursion
///   only), so `current_jit_closure()` matches every frame's
///   closure pointer.
/// - The chain is non-vararg (`!cl.proto.is_vararg`) — helper does
///   NOT reconstruct the vararg rotation that `push_frame` does.
/// - Every inlined `Op::Call` has `C == 2` (one return value);
///   `m.nresults` is therefore always 1. The helper writes whatever
///   the metadata says, no validation.
///
/// Safety:
/// - Caller runs under an `enter_jit(vm, Some(cl))` guard so
///   `current_jit_vm()` / `current_jit_closure()` return live
///   references.
/// - `metas` points to a valid array of length `n` of
///   `FrameMaterializeInfo`, alive for the duration of the call —
///   today it's a pointer into the owning `CompiledTrace.frame_metas`
///   `Box`, which lives at least as long as the trace's mmap.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_trace_materialize_frames(
    n: u64,
    metas: *const luna_core::jit::trace::FrameMaterializeInfo,
) -> i64 {
    // Count every entry to this helper from trace mcode.
    // Relaxed ordering: the counter is purely diagnostic; the read
    // side runs after process work has quiesced.
    TRACE_MATERIALIZE_FRAMES_FIRES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // Honour the existing deopt protocol: if any earlier helper in
    // this JIT entry parked a deopt, don't push frames — the
    // dispatcher will unwind via the deopt path.
    if vm.jit.pending_err.is_some() {
        return -1;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let head_frame = match vm.jit_last_lua_frame() {
        Some(f) => f,
        // No live Lua frame at trace head — shouldn't happen under
        // any current dispatcher path, but treat as deopt rather
        // than panic from the JIT.
        None => return -1,
    };
    let max_stack = cl.proto.max_stack as u32;
    for i in 0..n as usize {
        // SAFETY: caller-supplied `metas` points to a valid array of
        // length `n` per the contract above.
        let m = unsafe { *metas.add(i) };
        let new_base = head_frame.base + m.base_offset;
        vm.jit_ensure_stack((new_base + max_stack) as usize);
        vm.jit_push_inlined_frame(cl, new_base, m.pc, m.nresults);
    }
    0
}
