//! Traces shared between the Vms of one [`crate::Engine`]: publishing the
//! traces a Vm compiles, and installing them in another Vm that reaches
//! code of the same content.

use super::image::{Captured, Content, Settings, Source, Tier, TraceImage};
use super::reloc::Code;
use super::*;
use crate::jit_backend::engine::HeadKey;
use crate::jit_backend::storage::CraneliftJitStorage;
use luna_core::jit::trace_types::{AdoptRequest, AdoptedTrace, CompiledTrace, entry_tags_admit};
use std::sync::Arc;

/// What a baseline trace moves to the optimizing tier from: its
/// instructions, this Vm's values of their relocations, and the image it
/// shares (to take the optimizing tier's code from, or give it to).
pub(crate) struct TierSource {
    pub(crate) lir: Arc<lir::Lir>,
    pub(crate) relocs: Vec<(RelocKind, i64)>,
    pub(crate) image: Option<Arc<TraceImage>>,
}

/// Hands `ct`, just compiled from `record` with `cap`'s code, to the
/// engine of the Vm `storage` belongs to, if it has one.
pub(crate) fn publish(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    ct: &CompiledTrace,
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
                .traces
                .borrow()
                .iter()
                .find(|t| t.head_pc == pc)
                .map(|t| t.entry as usize);
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
    if let Some(t) = &ct.tier_up
        && let Some(src) = t.source.borrow_mut().as_mut()
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

/// [`luna_core::jit::TraceCompiler::adopt_traces`].
pub(crate) fn adopt(
    storage: &mut dyn luna_core::jit::JitStorage,
    req: &AdoptRequest<'_>,
) -> Vec<AdoptedTrace> {
    let Ok(cs) = crate::jit_backend::storage::from_storage(storage) else {
        return Vec::new();
    };
    let Some(engine) = cs.engine.clone() else {
        return Vec::new();
    };
    let parent_id = match req.side_parent {
        Some((pc, _)) => {
            let parent = req
                .proto
                .traces
                .borrow()
                .iter()
                .find(|t| t.head_pc == pc)
                .map(|t| t.entry as usize);
            match parent.and_then(|e| cs.images.get(&e).copied()) {
                Some(id) => Some(id),
                None => return Vec::new(),
            }
        }
        None => None,
    };
    let head = Content::of(&req.proto);
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
                img.protos[0] == head
                    && entry_tags_admit(&img.meta.entry_tags, req.entry_tags)
                    && img.side_parent.map(|(pc, exit, id)| (pc, exit, Some(id)))
                        == req.side_parent.map(|(pc, exit)| (pc, exit, parent_id))
            })
        }) else {
            return Vec::new();
        };
        let mut todo = vec![img.clone()];
        let mut i = 0;
        while i < todo.len() {
            if let Some(kids) = cache.children.get(&todo[i].id) {
                todo.extend(kids.iter().cloned());
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
        if let Some(a) = install(cs, req, img) {
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
/// is not loaded here.
fn install(
    cs: &mut CraneliftJitStorage,
    req: &AdoptRequest<'_>,
    img: &Arc<TraceImage>,
) -> Option<AdoptedTrace> {
    let mut protos = vec![req.proto];
    for c in &img.protos[1..] {
        protos.push(find_proto(req.roots, c)?);
    }
    let optimized = img.optimized.get().or(match &img.code {
        Some((Tier::Optimizing, c)) => Some(c),
        _ => None,
    });
    let mut ct = img
        .meta
        .instantiate(optimized.is_some(), req.proto.call_hot_count.get());
    let relocs: Vec<(RelocKind, i64)> = img
        .sources
        .iter()
        .map(|s| Some((kind_of(s), value_of(s, &protos, req, &ct)?)))
        .collect::<Option<_>>()?;
    let code: Option<&Code> = optimized.or(img.code.as_ref().map(|(_, c)| c));
    if let Some(code) = code {
        let vals: Vec<i64> = relocs.iter().map(|r| r.1).collect();
        let entry = cs.baseline_code.place(&code.relocated(&vals)).ok()?;
        // SAFETY: the code is a copy of a trace compiled for code of this
        // content, with the `TraceFn` ABI, and this Vm's addresses written
        // where it held the compiling Vm's; it stays mapped until this Vm
        // releases its code
        ct.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) };
        cs.images.insert(entry as usize, img.id);
        if optimized.is_none()
            && let (Some(t), Some(lir)) = (&ct.tier_up, &img.lir)
        {
            *t.source.borrow_mut() = Some(Box::new(TierSource {
                lir: lir.clone(),
                relocs,
                image: Some(img.clone()),
            }));
        }
    }
    Some(AdoptedTrace {
        trace: ct,
        side_parent: img.side_parent.map(|(pc, exit, _)| (pc, exit)),
        inlined: protos[1..].to_vec(),
    })
}

fn kind_of(s: &Source) -> RelocKind {
    match s {
        Source::Const { .. } | Source::MetaName(_) => RelocKind::Str,
        Source::Proto(_) => RelocKind::Proto,
        Source::Chain(n) => RelocKind::Chain(*n),
        Source::TierCell => RelocKind::TierCell,
    }
}

/// This Vm's address for `s`.
fn value_of(
    s: &Source,
    protos: &[Gc<Proto>],
    req: &AdoptRequest<'_>,
    ct: &CompiledTrace,
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
            as *const luna_core::jit::trace_types::FrameMaterializeInfo
            as i64,
        // the optimizing tier's code does not count
        Source::TierCell => ct
            .tier_up
            .as_ref()
            .map_or(0, |t| &*t.count as *const TCellU32 as i64),
    })
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

/// The optimizing tier's code for `ct`: taken from the image it shares when
/// some Vm compiled it already, else compiled (and given to the image).
pub(crate) fn tier_up(
    storage: &mut dyn luna_core::jit::JitStorage,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let source = ct.tier_up.as_ref()?.source.borrow_mut().take()?;
    let src = source.downcast::<TierSource>().ok()?;
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    if let Some(code) = src.image.as_ref().and_then(|i| i.optimized.get()) {
        let vals: Vec<i64> = src.relocs.iter().map(|r| r.1).collect();
        let entry = cs.baseline_code.place(&code.relocated(&vals)).ok()?;
        // SAFETY: as in `install`: the optimizing tier's code for this
        // trace, with this Vm's addresses written in
        return Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) });
    }
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let fn_id = lir::define_clif(&src.lir, &src.relocs, &mut *module)?;
    module.finalize_definitions().ok()?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    let ptr = module.get_finalized_function(fn_id);
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
