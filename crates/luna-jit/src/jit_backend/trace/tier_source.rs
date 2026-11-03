//! What a baseline trace moves to the optimizing tier from.

use super::image::TraceImage;
use super::*;
use crate::jit_backend::storage::CraneliftJitStorage;
use std::sync::Arc;

/// What a baseline trace moves to the optimizing tier from: its
/// instructions, this Vm's values of their relocations, the image it
/// shares (to take the optimizing tier's code from, or give it to), and
/// the record it was lowered from, when there is one, to lower it again
/// for the optimizing tier ([`TierSource::optimizing_lir`]).
pub(crate) struct TierSource {
    pub(crate) lir: Arc<lir::Lir>,
    pub(crate) relocs: Vec<(RelocKind, i64)>,
    pub(crate) image: Option<Arc<TraceImage>>,
    pub(crate) record: Option<TierRecord>,
}

/// A trace's record and the options it was lowered with.
pub(crate) struct TierRecord {
    pub(crate) record: TraceRecord,
    pub(crate) opts: CompileOptions,
    pub(crate) float_only: bool,
}

// SAFETY: the record's function handles are only read, on the thread of
// the Vm that owns the trace (the tier-up runs there; only the lowered
// instructions go to the LLVM compile thread), and the trace keeps the
// functions alive as long as it holds the record
unsafe impl Send for TierRecord {}
// SAFETY: as above; nothing mutates the record
unsafe impl Sync for TierRecord {}

impl TierSource {
    /// The instructions for the optimizing tier, with their relocations:
    /// the record lowered again keeping values in registers to the exits
    /// (`Lower::at_exits`), or the baseline tier's when there is no record
    /// (a trace taken from another Vm's image) or the two lowerings'
    /// relocations do not line up.
    pub(crate) fn optimizing_lir(&self) -> (Arc<lir::Lir>, Vec<(RelocKind, i64)>) {
        if let Some(r) = &self.record {
            // the baseline tier's options: the same relocations (its
            // iteration count among them), which the optimizing tier's
            // code generators read past
            if let Some((lir, _)) = lower::lower_trace_lir(&r.record, r.opts, r.float_only, true) {
                // the relocations name the same things in the same order;
                // their values are the baseline trace's, which the trace
                // keeps alive (this lowering's own frame chains go with
                // the trace it made, dropped here)
                let same = lir.relocs.len() == self.relocs.len()
                    && lir.relocs.iter().zip(&self.relocs).all(|(a, b)| a.0 == b.0);
                let owned = same.then(|| Arc::new(lir.detach()));
                lir.give();
                if let Some(owned) = owned {
                    return (owned, self.relocs.clone());
                }
            }
        }
        (self.lir.clone(), self.relocs.clone())
    }
}

/// The optimizing tier's code for `ct`: taken from the image it shares when
/// some Vm compiled it already, else compiled (and given to the image).
pub(crate) fn tier_up(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let source = luna_core::__private::take_tier_source(ct)?;
    let src = source.downcast::<TierSource>().ok()?;
    clif_tier_up(storage, &src, ct)
}

/// Cranelift's code for the trace `src` holds. For a trace whose back edge
/// calls the optimizing tier's loop (see `lower::handoff`), two functions:
/// the loop, which goes into `ct`'s loop cell, and the first pass, which
/// calls it and is returned as the entry.
pub(crate) fn clif_tier_up(
    storage: &mut dyn luna_core::__private::JitStorage,
    src: &TierSource,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let head_pc = ct.head_pc();
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    let at_loop = src.lir.loop_entry.is_some();
    if let Some(img) = &src.image
        && let Some(code) = img.optimized.get()
    {
        let place = |cs: &mut CraneliftJitStorage, code: &reloc::Code| {
            cs.baseline_code
                .place_with(code.bytes.len(), |out| {
                    code.write_to(out, |k| src.relocs[k].1)
                })
                .ok()
        };
        if at_loop {
            let p = place(cs, img.optimized_loop.get()?)?;
            // SAFETY: the image's optimizing tier's loop of this trace, with
            // this Vm's addresses written in
            unsafe { set_loop(ct, p) };
        }
        let entry = place(cs, code)?;
        super::code_dump::dump_len("tier-up-image", head_pc, entry, code.bytes.len());
        // SAFETY: as in `install`: the optimizing tier's code for this
        // trace, with this Vm's addresses written in
        return Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) });
    }
    let (lir, relocs) = src.optimizing_lir();
    // the loop first, each function in a module of its own
    let looped = match at_loop {
        true => Some(compile_one(cs, &lir, &relocs, lir::Start::Loop, head_pc)?),
        false => None,
    };
    if let Some((p, _)) = looped {
        // SAFETY: the loop `define_clif` compiled for this trace, in the
        // module the storage now keeps
        unsafe { set_loop(ct, p) };
    }
    let start = if at_loop {
        lir::Start::FirstPass
    } else {
        lir::Start::Whole
    };
    let (ptr, sites) = compile_one(cs, &lir, &relocs, start, head_pc)?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    if let (Some(img), Some((len, sites))) = (&src.image, sites) {
        // SAFETY: `ptr..ptr + len` is the function just finalized, which
        // the storage keeps mapped; so is the loop's
        let code = unsafe { reloc::copy_code(ptr, len, sites) };
        let lcode = match looped {
            // SAFETY: as for `code`: the loop's function, just finalized too
            Some((p, Some((n, s)))) => Some(unsafe { reloc::copy_code(p, n, s) }),
            _ => None,
        };
        let size = code.bytes.len() + lcode.as_ref().map_or(0, |c| c.bytes.len());
        let ok = match lcode {
            Some(l) => img.optimized_loop.set(l).is_ok(),
            None => !at_loop,
        };
        if ok
            && img.optimized.set(code).is_ok()
            && let Some(engine) = &cs.engine
        {
            engine.cache().grew(size);
        }
    }
    // SAFETY: `define_clif` declares the `TraceFn` signature, `(i64) -> i64`
    // in the platform calling convention, and `storage` now owns the module
    Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(ptr) })
}

/// The function `start` names compiled by Cranelift from `lir` in a module
/// `cs` keeps: its code, and where its relocations sit.
fn compile_one(
    cs: &mut CraneliftJitStorage,
    lir: &lir::Lir,
    relocs: &[(RelocKind, i64)],
    start: lir::Start,
    head_pc: u32,
) -> Option<(*const u8, Option<(usize, Vec<reloc::Site>)>)> {
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let id = lir::define_clif(lir, relocs, &mut *module, start)?;
    let sites = reloc::take_sites();
    module.finalize_definitions().ok()?;
    let ptr = module.get_finalized_function(id);
    let kind = match start {
        lir::Start::Loop => "tier-up-loop",
        _ => "tier-up",
    };
    super::code_dump::dump(kind, head_pc, ptr);
    cs.trace_handles.push(TraceHandle {
        _module: module.publish(),
        _entry_raw: ptr,
    });
    Some((ptr, sites))
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
