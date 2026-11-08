//! A trace's baseline code compiled again once a side trace is first wired
//! to one of its calling exits: the code its lowering left those exits'
//! calls out of (see `lower::relinks`) made with them, or taken from the
//! image that another Vm made it for.

use super::share::TierSource;
use super::*;

/// [`luna_core::__private::TraceCompiler::compile_calls`].
pub(crate) fn compile_calls(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let cells = ct.side_cells_at()? as i64;
    luna_core::__private::with_calls_source(ct, |src| {
        let src = src?.downcast_ref::<TierSource>()?;
        place(storage, src, cells, ct.head_pc())
    })
}

/// The baseline code of the trace `src` holds, its first cell at `cells`,
/// with its calls.
fn place(
    storage: &mut dyn luna_core::__private::JitStorage,
    src: &TierSource,
    cells: i64,
    head_pc: u32,
) -> Option<TraceFn> {
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    let img = src.image.as_ref();
    let entry = match img.and_then(|i| i.with_calls()) {
        Some(code) => {
            let at = |k: usize| src.relocs.get(k).map_or(cells, |r| r.1);
            let p = cs
                .baseline_code
                .place_with(code.bytes.len(), |out| code.write_to(out, at));
            p.ok()?
        }
        None => {
            let l = lir::expand(&src.lir, &src.relocs, cells, &[], 0);
            let (entry, code) = lir::assemble(&l, &mut cs.baseline_code, img.is_some()).ok()?;
            if let (Some(img), Some(code)) = (img, code) {
                let size = code.bytes.len();
                if img.keep_with_calls(code)
                    && let Some(engine) = &cs.engine
                {
                    engine.cache().grew(size);
                }
            }
            entry
        }
    };
    if let Some(img) = img {
        cs.images.insert(entry as usize, img.id);
    }
    super::code_dump::dump("calls", head_pc, entry);
    Some(baseline_entry(entry))
}
