//! The optimizing tier's code a shared image keeps: placed in this Vm's
//! code memory, or kept for the other Vms.

use super::*;

/// The optimizing tier's code `o` a shared image keeps for the trace `ct`,
/// placed in this Vm's code memory with relocation `k` holding `at(k)`: the
/// entry, and the loop into `ct`'s loop cell for a trace with one
/// (`at_loop`).
pub(super) fn from_image(
    cs: &mut CraneliftJitStorage,
    at: impl Fn(usize) -> i64 + Copy,
    at_loop: bool,
    o: &Optimized,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let head_pc = ct.head_pc();
    let place = |cs: &mut CraneliftJitStorage, code: &reloc::Code| {
        (cs.baseline_code)
            .place_with(code.bytes.len(), |out| code.write_to(out, at))
            .ok()
    };
    // the loop and the entry turn executable at once
    cs.baseline_code.open();
    let looped = match at_loop {
        true => o.looped.as_ref().and_then(|c| place(cs, c)).map(Some),
        false => Some(None),
    };
    let entry = place(cs, &o.entry);
    cs.baseline_code.seal().ok()?;
    let (looped, entry) = (looped?, entry?);
    if let Some(p) = looped {
        // SAFETY: the image's optimizing tier's loop of this trace, with
        // this Vm's addresses written in
        unsafe { set_loop(ct, p) };
    }
    super::code_dump::dump_len("tier-up-image", head_pc, entry, o.entry.bytes.len());
    // SAFETY: as in `install`: the optimizing tier's code for this
    // trace, with this Vm's addresses written in
    Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) })
}

/// The optimizing tier's code just compiled for the calling exits `calls`,
/// the entry and, for a trace with one (`at_loop`), the loop, kept in the
/// shared image `img` for other Vms to take up.
pub(super) fn publish(
    cs: &CraneliftJitStorage,
    img: &TraceImage,
    calls: &lir::Calls,
    (entry, looped): (reloc::Code, Option<reloc::Code>),
    at_loop: bool,
) {
    if at_loop && looped.is_none() {
        return;
    }
    let size = entry.bytes.len() + looped.as_ref().map_or(0, |c| c.bytes.len());
    let calls = calls.clone();
    if img.keep_optimized(Optimized {
        calls,
        entry,
        looped,
    }) && let Some(engine) = &cs.engine
    {
        engine.cache().grew(size);
    }
}

/// Puts the optimizing tier's loop at `p` into `ct`'s loop cell.
///
/// # Safety
/// `p` is the optimizing tier's loop of `ct`'s code, mapped as long as the
/// Vm holds `ct`.
pub(crate) unsafe fn set_loop(ct: &CompiledTrace, p: *const u8) {
    if let Some(t) = ct.tier_up() {
        // SAFETY: by the caller, with the `TraceFn` ABI the loop is compiled
        // with (`lir::define_clif`, `lir::compile_llvm`)
        unsafe {
            luna_core::__private::set_tier_up_loop(t, std::mem::transmute::<*const u8, TraceFn>(p))
        };
    }
}
