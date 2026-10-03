//! Frame materialization for inline side exits.

use crate::current_jit_vm;

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
/// - `closures[i]` is the closure frame `i` runs: the value its caller
///   called, which the trace checked is a Lua closure of the function it
///   inlined there.
/// - No inlined function is vararg — helper does
///   NOT reconstruct the vararg rotation that `push_frame` does.
/// - Every inlined `Op::Call` wants at most one result; `m.nresults` is
///   the count the caller asked for. The helper writes whatever the
///   metadata says, no validation.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread
/// opened with the running closure; `metas` points at `n` readable
/// `FrameMaterializeInfo`s (the trace passes its own `frame_metas`, which
/// live as long as its code) and `closures` at `n` readable payloads of
/// live Lua closures.
// SAFETY: no other item in the link is named `luna_jit_trace_materialize_frames`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_trace_materialize_frames(
    n: u64,
    metas: *const luna_core::jit::trace::FrameMaterializeInfo,
    closures: *const i64,
) -> i64 {
    // Count every entry to this helper from trace mcode.
    // Relaxed ordering: the counter is purely diagnostic; the read
    // side runs after process work has quiesced.
    TRACE_MATERIALIZE_FRAMES_FIRES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    // Honour the existing deopt protocol: if any earlier helper in
    // this JIT entry parked a deopt, don't push frames — the
    // dispatcher will unwind via the deopt path.
    if vm.jit.pending_err.is_some() {
        return -1;
    }
    let head_frame = match vm.jit_last_lua_frame() {
        Some(f) => f,
        // No live Lua frame at trace head — shouldn't happen under
        // any current dispatcher path, but treat as deopt rather
        // than panic from the JIT.
        None => return -1,
    };
    for i in 0..n as usize {
        // SAFETY: `i < n`, and `metas` and `closures` point at `n`
        // readable entries (# Safety)
        let (m, raw) = unsafe { (*metas.add(i), *closures.add(i)) };
        // SAFETY: `raw` is the payload of a live Lua closure (# Safety)
        let cl =
            unsafe { luna_core::runtime::Gc::from_ptr(raw as *mut luna_core::runtime::LuaClosure) };
        let new_base = head_frame.base + m.base_offset;
        vm.jit_ensure_stack((new_base + cl.proto.max_stack as u32) as usize);
        vm.jit_push_inlined_frame(cl, new_base, m.pc, m.nresults);
    }
    0
}
