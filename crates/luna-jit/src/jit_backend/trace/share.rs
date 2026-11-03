//! Traces shared between the Vms of one [`crate::Engine`]: publishing the
//! traces a Vm compiles, and installing them in another Vm that reaches
//! code of the same content.

use super::image::{Captured, Content, Settings, Source, Tier, TraceImage};
use super::reloc::Code;
use super::*;
use crate::jit_backend::engine::HeadKey;
use crate::jit_backend::storage::CraneliftJitStorage;
use luna_core::__private::ProtoTraceInternals;
use luna_core::__private::entry_tags_admit;
use std::sync::Arc;

pub(crate) use super::tier_source::TierSource;

/// Hands `ct`, just compiled from `record` with `cap`'s code, to the
/// engine of the Vm `storage` belongs to, if it has one.
pub(crate) fn publish(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
    ct: &mut CompiledTraceParts,
    opts: CompileOptions,
    version: luna_core::version::LuaVersion,
    cap: Captured,
) {
    let Ok(cs) = crate::jit_backend::storage::from_storage(storage) else {
        return;
    };
    let Some(engine) = cs.engine.clone() else {
        return;
    };
    let side_parent = match record.side_trace_parent {
        Some((proto, pc, exit)) => {
            let parent = proto
                .traces()
                .iter()
                .find(|t| t.head_pc() == pc)
                .map(|t| t.entry() as usize);
            // a parent that is not shared leaves its side traces unshared
            let Some(id) = parent.and_then(|e| cs.images.get(&e).copied()) else {
                return;
            };
            Some((pc, exit, id))
        }
        None => None,
    };
    let settings = Settings::new(version, opts, record.is_call_triggered, record.settings);
    let has_code = cap.code.is_some();
    let Some(img) = image::build(record, ct, side_parent, cap, engine.next_id()) else {
        return;
    };
    let img = Arc::new(img);
    if has_code {
        cs.images.insert(ct.entry as usize, img.id);
    }
    if let Some(t) = &mut ct.tier_up
        && let Some(src) = t.source_mut().as_mut()
        && let Some(src) = src.downcast_mut::<TierSource>()
    {
        src.image = Some(img.clone());
    }
    let key = HeadKey {
        settings,
        content: img.protos[0].hash,
        head_pc: record.head_pc,
    };
    engine.cache().insert(key, img);
}

/// [`luna_core::__private::TraceCompiler::adopt_traces`].
pub(crate) fn adopt(
    storage: &mut dyn luna_core::__private::JitStorage,
    req: &AdoptRequest<'_>,
) -> Vec<AdoptedTrace> {
    let Ok(cs) = crate::jit_backend::storage::from_storage(storage) else {
        return Vec::new();
    };
    let Some(engine) = cs.engine.clone() else {
        return Vec::new();
    };
    let parent_id = match req.side_parent {
        Some((parent_proto, pc, _)) => {
            let parent = parent_proto
                .traces()
                .iter()
                .find(|t| t.head_pc() == pc)
                .map(|t| t.entry() as usize);
            match parent.and_then(|e| cs.images.get(&e).copied()) {
                Some(id) => Some(id),
                None => return Vec::new(),
            }
        }
        None => None,
    };
    let head = Content::of_ref(&req.proto);
    let settings = Settings::new(req.version, req.opts, req.call_triggered, req.settings);
    let key = HeadKey {
        settings,
        content: head.hash,
        head_pc: req.head_pc,
    };
    // the image, then its side traces, each after its parent
    let todo: Vec<Arc<TraceImage>> = {
        let cache = engine.cache();
        let Some(img) = cache.by_head.get(&key).and_then(|list| {
            list.iter().find(|img| {
                img.protos[0] == *head
                    && entry_tags_admit(&img.meta.entry_tags, req.entry_tags)
                    && img.side_parent.map(|(pc, exit, id)| (pc, exit, Some(id)))
                        == req.side_parent.map(|(_, pc, exit)| (pc, exit, parent_id))
            })
        }) else {
            return Vec::new();
        };
        let mut todo = vec![img.clone()];
        let mut i = 0;
        while i < todo.len() {
            if let Some(kids) = cache.children.get(&todo[i].id) {
                // a side trace of an exit inside a function the parent
                // inlined starts in that function: it is taken over when
                // that exit becomes hot here
                todo.extend(kids.iter().filter(|k| k.protos[0] == *head).cloned());
            }
            i += 1;
        }
        todo
    };
    let mut out = Vec::with_capacity(todo.len());
    let mut installed: Vec<u64> = Vec::with_capacity(todo.len());
    for (i, img) in todo.iter().enumerate() {
        // the first is what was asked for; a side trace of it goes in only
        // after its parent did
        if i > 0
            && !img
                .side_parent
                .is_some_and(|(_, _, p)| installed.contains(&p))
        {
            continue;
        }
        if let Some(a) = install(cs, req, img, i == 0) {
            installed.push(img.id);
            out.push(a);
        } else if i == 0 {
            return Vec::new();
        }
    }
    out
}

/// `img` as a trace of this Vm: its code copied into this Vm's code memory
/// with this Vm's addresses written in. `None` when a function it inlined
/// is not loaded here. `img_is_asked`: `img` is the trace `req` asks for,
/// not a side trace that comes with it.
fn install(
    cs: &mut CraneliftJitStorage,
    req: &AdoptRequest<'_>,
    img: &Arc<TraceImage>,
    img_is_asked: bool,
) -> Option<AdoptedTrace> {
    let mut protos = vec![req.proto];
    for c in &img.protos[1..] {
        protos.push(find_proto(req.roots, c)?);
    }
    // a trace whose back edge calls the optimizing tier's loop keeps a tier
    // move (its code reads the move's loop cell); its optimizing tier's code
    // is the first pass, the loop beside it
    let at_loop = img.sources.contains(&Source::TierLoop);
    let optimized = img.optimized.get().or(match &img.code {
        Some((Tier::Optimizing, c)) => Some(c),
        _ => None,
    });
    let looped = img.optimized_loop.get().filter(|_| at_loop && optimized.is_some());
    let mut ct = img
        .meta
        .instantiate(optimized.is_some() && !at_loop, req.proto.call_hot_count());
    let relocs: Vec<(RelocKind, i64)> = img
        .sources
        .iter()
        .map(|s| Some((kind_of(s), value_of(s, &protos, req, &ct)?)))
        .collect::<Option<_>>()?;
    let code: Option<&Code> = match (at_loop, looped) {
        (true, None) => img.code.as_ref().map(|(_, c)| c),
        _ => optimized.or(img.code.as_ref().map(|(_, c)| c)),
    };
    if let Some(code) = code {
        let entry = place(cs, code, &relocs)?;
        super::code_dump::dump_len("image", ct.head_pc, entry, code.bytes.len());
        // SAFETY: the code is a copy of a trace compiled for code of this
        // content, with the `TraceFn` ABI, and this Vm's addresses written
        // where it held the compiling Vm's; it stays mapped until this Vm
        // releases its code
        ct.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) };
        cs.images.insert(entry as usize, img.id);
        if let (Some(c), Some(t)) = (looped, &ct.tier_up) {
            let p = place(cs, c, &relocs)?;
            super::code_dump::dump_len("image-loop", ct.head_pc, p, c.bytes.len());
            // SAFETY: as above, the optimizing tier's loop of this trace,
            // which the first pass placed as the entry calls
            unsafe {
                luna_core::__private::set_tier_up_loop(
                    t,
                    std::mem::transmute::<*const u8, TraceFn>(p),
                )
            };
        }
        if let (None, Some(t), Some(lir)) = (looped.or(optimized.filter(|_| !at_loop)), &mut ct.tier_up, &img.lir) {
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

fn kind_of(s: &Source) -> RelocKind {
    match s {
        Source::Const { .. } | Source::MetaName(_) => RelocKind::Str,
        Source::Proto(_) => RelocKind::Proto,
        Source::Chain(n) => RelocKind::Chain(*n),
        Source::TierCell => RelocKind::TierCell,
        Source::TierLoop => RelocKind::TierLoop,
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
        Source::TierLoop => ct.tier_up.as_ref().map_or(0, |t| {
            luna_core::__private::tier_up_loop_cell(t) as *const TCellPtr as i64
        }),
    })
}

/// `code` copied into this Vm's code memory with the addresses `relocs`
/// written in.
fn place(cs: &mut CraneliftJitStorage, code: &Code, relocs: &[(RelocKind, i64)]) -> Option<*const u8> {
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
