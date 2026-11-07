//! Taking up a shared trace image: its code placed in this Vm's code
//! memory with this Vm's addresses written in.

use super::*;

/// `img` as a trace of this Vm: its code copied into this Vm's code memory
/// with this Vm's addresses written in. `None` when a function it inlined
/// is not loaded here. `img_is_asked`: `img` is the trace `req` asks for,
/// not a side trace that comes with it.
pub(super) fn install(
    cs: &mut CraneliftJitStorage,
    req: &AdoptRequest<'_>,
    img: &Arc<TraceImage>,
    img_is_asked: bool,
) -> Option<AdoptedTrace> {
    let mut protos = vec![req.proto];
    for c in &img.protos[1..] {
        protos.push(find_proto(req.roots, c)?);
    }
    let pick = pick_code(img);
    let mut ct = img
        .meta
        .instantiate(pick.tiered, req.proto.call_hot_count());
    let relocs: Vec<(RelocKind, i64)> = img
        .sources
        .iter()
        .map(|s| Some((kind_of(s), value_of(s, &protos, req, &ct)?)))
        .collect::<Option<_>>()?;
    if let Some(code) = pick.entry {
        let entry = place(cs, code, &relocs)?;
        super::code_dump::dump_len("image", ct.head_pc, entry, code.bytes.len());
        // SAFETY: the code is a copy of a trace compiled for code of this
        // content, with the `TraceFn` ABI, and this Vm's addresses written
        // where it held the compiling Vm's; it stays mapped until this Vm
        // releases its code
        ct.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) };
        cs.images.insert(entry as usize, img.id);
        if let (Some(c), Some(t)) = (pick.looped, &ct.tier_up) {
            place_loop(cs, c, &relocs, t, ct.head_pc)?;
        }
        if let (true, Some(t), Some(lir)) = (pick.keep_source, &mut ct.tier_up, &img.lir) {
            *t.source_mut() = Some(Box::new(TierSource {
                lir: lir.clone(),
                relocs,
                image: Some(img.clone()),
                record: None,
            }));
        }
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
    })
}

/// Which of an image's code a Vm taking it up runs.
struct Pick<'a> {
    /// The entry: the optimizing tier's (its first pass when it has a
    /// loop), else the baseline code.
    entry: Option<&'a Code>,
    /// The optimizing tier's loop, beside its first pass.
    looped: Option<&'a Code>,
    /// The entry is the optimizing tier's whole trace: no tier move.
    tiered: bool,
    /// The trace keeps its source for the optimizing tier to compile.
    keep_source: bool,
}

/// A trace whose back edge calls the optimizing tier's loop keeps a tier
/// move (its code reads the move's loop cell); its optimizing tier's code
/// is the first pass, the loop beside it.
fn pick_code(img: &TraceImage) -> Pick<'_> {
    let at_loop = img.sources.contains(&Source::TierLoop);
    let baseline = img.code.as_ref().map(|(_, c)| c);
    let optimized = img.optimized.get().or(match &img.code {
        Some((Tier::Optimizing, c)) => Some(c),
        _ => None,
    });
    let looped = img
        .optimized_loop
        .get()
        .filter(|_| at_loop && optimized.is_some());
    Pick {
        entry: match (at_loop, looped) {
            (true, None) => baseline,
            _ => optimized.or(baseline),
        },
        looped,
        tiered: optimized.is_some() && !at_loop,
        keep_source: looped.or(optimized.filter(|_| !at_loop)).is_none(),
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
        Source::SideCell(n) => RelocKind::SideCell(*n),
    }
}

/// This Vm's address for `s`.
fn value_of(
    s: &Source,
    protos: &[Gc<Proto>],
    req: &AdoptRequest<'_>,
    ct: &CompiledTraceParts,
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
        Source::SideCell(n) => &*ct.side_cells.get(*n as usize)?.1 as *const SideCell as i64,
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
