//! What a baseline trace moves to the optimizing tier from.

use super::image::{Optimized, TraceImage};
use super::*;
use crate::jit_backend::storage::CraneliftJitStorage;
use luna_core::__private::TierKept;
use std::sync::Arc;

mod place;
pub(crate) use place::set_loop;

/// What a baseline trace moves to the optimizing tier from: its
/// instructions, this Vm's values of their relocations, the image it
/// shares (to take the optimizing tier's code from, or give it to), and
/// the record it was lowered from, when there is one, to lower it again
/// for the optimizing tier ([`TierSource::into_optimizing`]).
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
    /// The source with the instructions for the optimizing tier and no
    /// record: the record lowered again keeping values in registers to the
    /// exits (`Lower::at_exits`), or the baseline tier's instructions when
    /// there is no record (a trace taken from another Vm's image) or the
    /// two lowerings' relocations do not line up.
    pub(crate) fn into_optimizing(mut self: Box<Self>) -> Box<TierSource> {
        let Some(r) = self.record.take() else {
            return self;
        };
        // the baseline tier's options: the same relocations (its iteration
        // count among them), which the optimizing tier's code generators
        // read past
        if let Some((lir, _)) = lower::lower_trace_lir(&r.record, r.opts, r.float_only, true) {
            // the relocations name the same things in the same order;
            // their values are the baseline trace's, which the trace keeps
            // alive (this lowering's own frame chains go with the trace it
            // made, dropped here)
            // and the same calling exits, whose cells the trace has
            let cells = |l: &lir::Lir| l.relinks.as_ref().map(|x| x.cells.clone());
            let same = lir.relocs.len() == self.relocs.len()
                && lir.relocs.iter().zip(&self.relocs).all(|(a, b)| a.0 == b.0)
                && cells(&lir) == cells(&self.lir);
            let owned = same.then(|| Arc::new(lir.detach()));
            lir.give();
            if let Some(owned) = owned {
                self.lir = owned;
            }
        }
        self
    }
}

/// The instructions of `src` with their calls made (see `lir::expand`) when
/// one of `ct`'s calling exits calls its side trace as `calls` says (`ct`'s
/// cells made by then); `None` when none does.
pub(crate) fn expanded(
    src: &TierSource,
    ct: &CompiledTrace,
    calls: &lir::Calls,
    run_words: i64,
) -> Option<lir::Lir> {
    let cells = cells_for(ct, calls)?;
    Some(lir::expand(
        &src.lir,
        &src.relocs,
        cells,
        calls.plans(),
        run_words,
    ))
}

/// The address of the run words of the Vm `storage` is.
#[cfg(feature = "llvm-jit")]
pub(crate) fn run_words_of(storage: &mut dyn luna_core::__private::JitStorage) -> i64 {
    let cs = crate::jit_backend::storage::from_storage(storage);
    cs.expect("the optimizing tier compiles through Cranelift's storage")
        .run_words
}

/// The address of `ct`'s first cell when a calling exit calls as `calls`
/// says.
fn cells_for(ct: &CompiledTrace, calls: &lir::Calls) -> Option<i64> {
    let any = match calls {
        lir::Calls::Held(h) => h.iter().any(Option::is_some),
        _ => false,
    };
    any.then(|| ct.side_cells_at().map(|c| c as i64)).flatten()
}

/// The instructions and the relocations' values to compile from: those of
/// `ex` when made, else `src`'s.
pub(crate) fn of<'a>(
    src: &'a TierSource,
    ex: &'a Option<lir::Lir>,
) -> (&'a lir::Lir, &'a [(RelocKind, i64)]) {
    match ex {
        Some(l) => (l, &l.relocs),
        None => (&src.lir, &src.relocs),
    }
}

/// The calling exits of `ct`'s code that call its side traces from the
/// optimizing tier's code: those whose side cell holds one now.
pub(crate) fn calls_of(ct: &CompiledTrace) -> lir::Calls {
    lir::Calls::Held(ct.side_cell_plans().into())
}

/// What the trace keeps of `src` to compile again once a side cell left
/// empty by `calls` gets a side trace; nothing when none is left empty.
/// `stale`: a cell got one since `calls` was read.
pub(crate) fn kept(src: Box<TierSource>, calls: &lir::Calls, stale: bool) -> Option<TierKept> {
    let lir::Calls::Held(held) = calls else {
        return None;
    };
    held.iter()
        .any(Option::is_none)
        .then(|| TierKept { source: src, stale })
}

/// The optimizing tier's code for `ct`: taken from the image it shares when
/// some Vm compiled it for the same calling exits already, else compiled
/// (and given to the image); with what the trace keeps to compile it again.
pub(crate) fn tier_up(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
) -> (Option<TraceFn>, Option<TierKept>) {
    let Some(source) = luna_core::__private::take_tier_source(ct) else {
        return (None, None);
    };
    let Ok(src) = source.downcast::<TierSource>() else {
        return (None, None);
    };
    let src = src.into_optimizing();
    let calls = calls_of(ct);
    let entry = clif_tier_up(storage, &src, ct, &calls);
    let kept = entry.and_then(|_| kept(src, &calls, false));
    (entry, kept)
}

/// Cranelift's code for the trace `src` holds, its calling exits calling
/// as `calls` says. For a trace whose back edge calls the optimizing
/// tier's loop (see `lower::handoff`), two functions: the loop, which goes
/// into `ct`'s loop cell, and the first pass, which calls it and is
/// returned as the entry.
pub(crate) fn clif_tier_up(
    storage: &mut dyn luna_core::__private::JitStorage,
    src: &TierSource,
    ct: &CompiledTrace,
    calls: &lir::Calls,
) -> Option<TraceFn> {
    let head_pc = ct.head_pc();
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    let at_loop = src.lir.loop_entry.is_some();
    if let Some(o) = src.image.as_ref().and_then(|img| img.optimized(calls)) {
        // the relocations past the image's: the first side cell, then the
        // run words
        let cells = cells_for(ct, calls).unwrap_or(0);
        let rw = cs.run_words;
        let n = src.relocs.len();
        let at = |k: usize| {
            src.relocs
                .get(k)
                .map_or(if k == n { cells } else { rw }, |r| r.1)
        };
        return place::from_image(cs, at, at_loop, &o, ct);
    }
    let ex = expanded(src, ct, calls, cs.run_words);
    let (lir, relocs) = of(src, &ex);
    // the loop first, each function in a module of its own
    let looped = match at_loop {
        true => Some(compile_loop(cs, lir, relocs, ct, calls)?),
        false => None,
    };
    let start = if at_loop {
        lir::Start::FirstPass
    } else {
        lir::Start::Whole
    };
    let (ptr, sites) = compile_one(cs, lir, relocs, (start, calls), head_pc)?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    if let (Some(img), Some((len, sites))) = (&src.image, sites) {
        // SAFETY: `ptr..ptr + len` is the function just finalized, which
        // the storage keeps mapped; so is the loop's
        let entry = unsafe { reloc::copy_code(ptr, len, sites) };
        let looped = match looped {
            // SAFETY: as for `entry`: the loop's function, just finalized too
            Some((p, Some((n, s)))) => Some(unsafe { reloc::copy_code(p, n, s) }),
            _ => None,
        };
        place::publish(cs, img, calls, (entry, looped), at_loop);
    }
    // SAFETY: `define_clif` declares the `TraceFn` signature, `(i64) -> i64`
    // in the platform calling convention, and `storage` now owns the module
    Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(ptr) })
}

/// The optimizing tier's loop of the trace `ct`, compiled from `lir` in a
/// module `cs` keeps and put in `ct`'s loop cell.
fn compile_loop(
    cs: &mut CraneliftJitStorage,
    lir: &lir::Lir,
    relocs: &[(RelocKind, i64)],
    ct: &CompiledTrace,
    calls: &lir::Calls,
) -> Option<(*const u8, Option<(usize, Vec<reloc::Site>)>)> {
    let l = compile_one(cs, lir, relocs, (lir::Start::Loop, calls), ct.head_pc())?;
    // SAFETY: the loop `define_clif` compiled for this trace, in the module
    // the storage now keeps
    unsafe { set_loop(ct, l.0) };
    Some(l)
}

/// The function `start` names, its calling exits calling as `calls` says,
/// compiled by Cranelift from `lir` in a module `cs` keeps: its code, and
/// where its relocations sit.
fn compile_one(
    cs: &mut CraneliftJitStorage,
    lir: &lir::Lir,
    relocs: &[(RelocKind, i64)],
    (start, calls): (lir::Start, &lir::Calls),
    head_pc: u32,
) -> Option<(*const u8, Option<(usize, Vec<reloc::Site>)>)> {
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let id = lir::define_clif(lir, relocs, &mut *module, start, calls)?;
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
