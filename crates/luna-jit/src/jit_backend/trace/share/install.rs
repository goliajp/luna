//! Taking up a shared trace image: its code placed in this Vm's code
//! memory with this Vm's addresses written in.

use super::*;
use crate::jit_backend::trace::image::Optimized;
use luna_core::__private::side_cells;

/// `img` as a trace of this Vm: its code copied into this Vm's code memory
/// with this Vm's addresses written in. `None` when a function it inlined
/// is not loaded here. `img_is_asked`: `img` is the trace `req` asks for,
/// not a side trace that comes with it; `todo`: the images taken up with
/// it.
pub(super) fn install(
    cs: &mut CraneliftJitStorage,
    req: &AdoptRequest<'_>,
    img: &Arc<TraceImage>,
    todo: &[Arc<TraceImage>],
    img_is_asked: bool,
) -> Option<AdoptedTrace> {
    let mut protos = vec![req.proto];
    for c in &img.protos[1..] {
        protos.push(find_proto(req.roots, c)?);
    }
    let pick = pick_code(img, todo);
    let mut ct = img
        .meta
        .instantiate(pick.tiered && !pick.retiers, req.proto.call_hot_count());
    // code that holds the cells' address (the optimizing tier's, compiled
    // with every calling exit calling, or the baseline code with its
    // calls): they are made now
    if img.sources.contains(&Source::SideCells) || pick.with_calls.is_some() {
        ct.side_cells = Some(side_cells(ct.side_cell_exits.len()));
    }
    let relocs: Vec<(RelocKind, i64)> = img
        .sources
        .iter()
        .map(|s| Some((kind_of(s), value_of(s, &protos, req, &ct, cs.run_words)?)))
        .collect::<Option<_>>()?;
    let (entry, looped) = match &pick.fresh {
        Some(o) => (Some(&o.entry), o.looped.as_ref()),
        None => (pick.code, None),
    };
    if let Some(code) = entry {
        let (entry, kind, len) = match pick.with_calls {
            Some(c) => (place_with_calls(cs, c, &relocs, &ct)?, "image-calls", c),
            None => (place(cs, code, &relocs)?, "image", code),
        };
        super::code_dump::dump_len(kind, ct.head_pc, entry, len.bytes.len());
        // SAFETY: the code is a copy of a trace compiled for code of this
        // content, with the `TraceFn` ABI, and this Vm's addresses written
        // where it held the compiling Vm's; it stays mapped until this Vm
        // releases its code
        ct.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) };
        cs.images.insert(entry as usize, img.id);
        if let (Some(c), Some(t)) = (looped, &ct.tier_up) {
            place_loop(cs, c, &relocs, t, ct.head_pc)?;
        }
        give_source(&mut ct, &pick, img, relocs);
    }
    Some(AdoptedTrace {
        // SAFETY: the code is a copy of a trace compiled for code of this
        // content against the tags and window the image's metadata gives
        // `ct`, placed above with this Vm's addresses and `ct`'s cells; the
        // tier source is made above for it; no side-trace cell is set yet
        trace: unsafe { CompiledTrace::from_parts(ct) },
        side_parent: match (img.side_parent, req.side_parent) {
            (None, _) => None,
            // the side trace asked for: its parent is the one asked about
            (Some(_), Some(p)) if img_is_asked => Some(p),
            // one that comes with its parent starts in the same function
            (Some((pc, exit, _)), _) => Some((req.proto, pc, exit)),
        },
        inlined: protos[1..].to_vec(),
        ahead: false,
    })
}

/// Gives the trace `ct` taken up from `img` the source to compile it from:
/// for its tier move, to move, or, for code that is the optimizing tier's
/// already and calls from no exit, to compile again once a side cell gets a
/// side trace; without one, to compile its baseline code again then.
fn give_source(
    ct: &mut CompiledTraceParts,
    pick: &Pick<'_>,
    img: &Arc<TraceImage>,
    relocs: Vec<(RelocKind, i64)>,
) {
    let Some(lir) = &img.lir else {
        return;
    };
    let src = Box::new(TierSource {
        lir: lir.clone(),
        relocs,
        image: Some(img.clone()),
        record: None,
    });
    match (&mut ct.tier_up, pick.keep_source, pick.retiers) {
        (Some(t), true, _) => *t.source_mut() = Some(src),
        (Some(t), false, true) => *t.kept_mut() = Some(src),
        (None, ..) if lir.relinks.is_some() => ct.calls_source = Some(src),
        _ => {}
    }
}

/// Which of an image's code a Vm taking it up runs.
struct Pick<'a> {
    /// The optimizing tier's code compiled for no calling exit calling, as
    /// a trace taken up has none of its side cells filled yet: the entry
    /// (the first pass when it has a loop) and the loop beside it.
    fresh: Option<Arc<Optimized>>,
    /// The entry otherwise: the image's code.
    code: Option<&'a Code>,
    /// The entry is the optimizing tier's whole trace: no tier move.
    tiered: bool,
    /// The trace keeps its source for the optimizing tier to compile.
    keep_source: bool,
    /// The trace has side cells and the image the instructions to compile
    /// it again for them at the optimizing tier.
    retiers: bool,
    /// The baseline code with its calling exits' calls, run in place of
    /// `code`: side traces of the trace come with it, whose wiring would
    /// have the code made with its calls straight away.
    with_calls: Option<&'a Code>,
}

/// A trace whose back edge calls the optimizing tier's loop keeps a tier
/// move (its code reads the move's loop cell); its optimizing tier's code
/// is the first pass, the loop beside it. Its baseline code is the one
/// with its calls when some of `todo` is a side trace of it.
fn pick_code<'a>(img: &'a TraceImage, todo: &[Arc<TraceImage>]) -> Pick<'a> {
    let at_loop = img.sources.contains(&Source::TierLoop);
    let cells = img.meta.side_cells();
    let fresh = img
        .optimized(&lir::Calls::Held(vec![None; cells].into()))
        .filter(|o| !at_loop || o.looped.is_some());
    let direct = matches!(img.code, Some((Tier::Optimizing, _)));
    let tiered = !at_loop && (fresh.is_some() || direct);
    let has_side = || {
        todo.iter()
            .any(|k| k.side_parent.is_some_and(|p| p.2 == img.id))
    };
    let with_calls = match (&fresh, &img.code) {
        (None, Some((Tier::Baseline, _))) if cells > 0 && has_side() => img.with_calls(),
        _ => None,
    };
    Pick {
        keep_source: fresh.is_none() && !tiered,
        retiers: cells > 0 && img.lir.is_some(),
        with_calls,
        fresh,
        code: img.code.as_ref().map(|(_, c)| c),
        tiered,
    }
}

/// `c`, the optimizing tier's loop of the trace `t` moves, placed with
/// `relocs` and handed to `t`.
fn place_loop(
    cs: &mut CraneliftJitStorage,
    c: &Code,
    relocs: &[(RelocKind, i64)],
    t: &luna_core::__private::TierUp,
    head_pc: u32,
) -> Option<()> {
    let p = place(cs, c, relocs)?;
    super::code_dump::dump_len("image-loop", head_pc, p, c.bytes.len());
    // SAFETY: a copy of the optimizing tier's loop of this trace, with this
    // Vm's addresses written in, which the first pass placed as the entry
    // calls; it stays mapped until this Vm releases its code
    unsafe {
        luna_core::__private::set_tier_up_loop(t, std::mem::transmute::<*const u8, TraceFn>(p))
    };
    Some(())
}

fn kind_of(s: &Source) -> RelocKind {
    match s {
        Source::Const { .. } | Source::MetaName(_) => RelocKind::Str,
        Source::Proto(_) => RelocKind::Proto,
        Source::Chain(n) => RelocKind::Chain(*n),
        Source::TierCell => RelocKind::TierCell,
        Source::TierLoop => RelocKind::TierLoop,
        Source::SideCells => RelocKind::SideCells,
        Source::RunWords => RelocKind::RunWords,
    }
}

/// This Vm's address for `s`.
fn value_of(
    s: &Source,
    protos: &[Gc<Proto>],
    req: &AdoptRequest<'_>,
    ct: &CompiledTraceParts,
    run_words: i64,
) -> Option<i64> {
    Some(match s {
        Source::Const { p, k } => match protos[*p as usize].consts.get(*k as usize)? {
            luna_core::runtime::Value::Str(s) => s.as_ptr() as i64,
            _ => return None,
        },
        Source::MetaName(b) => {
            let s = req.mm_names.iter().find(|s| s.as_bytes() == &**b)?;
            s.as_ptr() as i64
        }
        Source::Proto(p) => protos[*p as usize].as_ptr() as i64,
        Source::Chain(n) => TArc::as_ptr(&ct.per_exit_inline.get(*n as usize)?.chain)
            as *const luna_core::__private::FrameMaterializeInfo as i64,
        // the optimizing tier's code does not count
        Source::TierCell => ct
            .tier_up
            .as_ref()
            .map_or(0, |t| t.count() as *const TCellU32 as i64),
        // only a trace that counts has a loop to hand over; without the
        // cell its code would read address 0, so the image is not taken up
        Source::TierLoop => {
            let t = ct.tier_up.as_ref()?;
            luna_core::__private::tier_up_loop_cell(t) as *const TCellPtr as i64
        }
        // this Vm's cell: every Vm wires its own side traces
        Source::SideCells => ct.side_cells.as_ref()?.as_ptr() as i64,
        Source::RunWords => run_words,
    })
}

/// `code` copied into this Vm's code memory with the addresses `relocs`
/// written in.
fn place(
    cs: &mut CraneliftJitStorage,
    code: &Code,
    relocs: &[(RelocKind, i64)],
) -> Option<*const u8> {
    cs.baseline_code
        .place_with(code.bytes.len(), |out| code.write_to(out, |k| relocs[k].1))
        .ok()
}

/// The baseline code with its calls `code` copied into this Vm's code
/// memory with the addresses `relocs` written in, and `ct`'s first side
/// cell past them.
fn place_with_calls(
    cs: &mut CraneliftJitStorage,
    code: &Code,
    relocs: &[(RelocKind, i64)],
    ct: &CompiledTraceParts,
) -> Option<*const u8> {
    let cells = ct.side_cells.as_ref()?.as_ptr() as i64;
    let at = |k: usize| relocs.get(k).map_or(cells, |r| r.1);
    cs.baseline_code
        .place_with(code.bytes.len(), |out| code.write_to(out, at))
        .ok()
}

/// A function of the chunks `roots` with content `c`.
fn find_proto(roots: &[Gc<Proto>], c: &Content) -> Option<Gc<Proto>> {
    let mut stack: Vec<Gc<Proto>> = roots.to_vec();
    while let Some(p) = stack.pop() {
        if c.matches(&p) {
            return Some(p);
        }
        stack.extend(p.protos.iter().copied());
    }
    None
}
