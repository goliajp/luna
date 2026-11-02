//! What a baseline trace moves to the optimizing tier from.

use super::image::TraceImage;
use super::*;
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
    clif_tier_up(storage, &src, ct.head_pc())
}

/// Cranelift's code for the trace `src` holds.
pub(crate) fn clif_tier_up(
    storage: &mut dyn luna_core::__private::JitStorage,
    src: &TierSource,
    head_pc: u32,
) -> Option<TraceFn> {
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    if let Some(code) = src.image.as_ref().and_then(|i| i.optimized.get()) {
        let n = code.bytes.len();
        let entry = cs
            .baseline_code
            .place_with(n, |out| code.write_to(out, |k| src.relocs[k].1))
            .ok()?;
        super::code_dump::dump_len("tier-up-image", head_pc, entry, n);
        // SAFETY: as in `install`: the optimizing tier's code for this
        // trace, with this Vm's addresses written in
        return Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) });
    }
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let (lir, relocs) = src.optimizing_lir();
    let fn_id = lir::define_clif(&lir, &relocs, &mut *module)?;
    module.finalize_definitions().ok()?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    let ptr = module.get_finalized_function(fn_id);
    super::code_dump::dump("tier-up", head_pc, ptr);
    let sites = reloc::take_sites();
    cs.trace_handles.push(TraceHandle {
        _module: module.publish(),
        _entry_raw: ptr,
    });
    if let (Some(img), Some((len, sites))) = (&src.image, sites) {
        // SAFETY: `ptr..ptr + len` is the function just finalized, which
        // the storage keeps mapped
        let code = unsafe { reloc::copy_code(ptr, len, sites) };
        let size = code.bytes.len();
        if img.optimized.set(code).is_ok()
            && let Some(engine) = &cs.engine
        {
            engine.cache().grew(size);
        }
    }
    // SAFETY: `define_clif` declares the `TraceFn` signature, `(i64) -> i64`
    // in the platform calling convention, and `storage` now owns the module
    Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(ptr) })
}
