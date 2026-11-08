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

mod install;
use install::install;

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
    if let Some(src) = ct.calls_source.as_mut()
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
    // the image, then its side traces, each after its parent; then, as
    // asked, those of the function's other heads, each followed by its side
    // traces
    let (todo, ahead_from) = {
        let cache = engine.cache();
        let Some(img) = cache.by_head.get(&key).and_then(|list| {
            list.iter().find(|img| {
                img.protos[0] == *head
                    && entry_tags_admit(img.meta.plans.entry_tags(), req.entry_tags)
                    && img.side_parent.map(|(pc, exit, id)| (pc, exit, Some(id)))
                        == req.side_parent.map(|(_, pc, exit)| (pc, exit, parent_id))
            })
        }) else {
            return Vec::new();
        };
        let mut todo = vec![img.clone()];
        with_side_traces(&cache, &mut todo, 0, &head);
        let ahead_from = todo.len();
        if req.ahead {
            let traces = req.proto.traces();
            for root in cache.roots_of(settings, &head, req.head_pc, |pc| {
                traces.iter().any(|t| t.head_pc() == pc)
            }) {
                let from = todo.len();
                todo.push(root);
                with_side_traces(&cache, &mut todo, from, &head);
            }
        }
        (todo, ahead_from)
    };
    // one batch: the code of all the traces turns executable at once
    cs.baseline_code.open();
    let out = install_all(cs, req, &todo, ahead_from);
    if cs.baseline_code.seal().is_err() {
        return Vec::new();
    }
    out
}

/// Adds to `todo` the side traces of `todo[from..]`, each after its parent,
/// that start in the function of content `head`.
fn with_side_traces(
    cache: &crate::jit_backend::engine::TraceCache,
    todo: &mut Vec<Arc<TraceImage>>,
    from: usize,
    head: &Content,
) {
    let mut i = from;
    while i < todo.len() {
        if let Some(kids) = cache.children.get(&todo[i].id) {
            let mut exits: Vec<usize> = Vec::new();
            for k in kids {
                // a side trace of an exit inside a function the parent
                // inlined starts in that function: it is taken over when
                // that exit becomes hot here
                if k.protos[0] != *head {
                    continue;
                }
                // Vms that took up the parent before any had a side trace
                // for an exit each compiled their own: the exit runs one,
                // the first cached
                if let Some((_, exit, _)) = k.side_parent
                    && !exits.contains(&exit)
                {
                    exits.push(exit);
                    todo.push(k.clone());
                }
            }
        }
        i += 1;
    }
}

/// Installs the images `todo`, the one asked for first: none when that one
/// cannot be. Those from `ahead_from` on start at other heads.
fn install_all(
    cs: &mut CraneliftJitStorage,
    req: &AdoptRequest<'_>,
    todo: &[Arc<TraceImage>],
    ahead_from: usize,
) -> Vec<AdoptedTrace> {
    let mut out = Vec::with_capacity(todo.len());
    let mut installed: Vec<u64> = Vec::with_capacity(todo.len());
    for (i, img) in todo.iter().enumerate() {
        let ahead = i >= ahead_from;
        // the first is what was asked for, and each of another head's goes
        // in as that was; a side trace goes in only after its parent did
        let root = i == 0 || (ahead && img.side_parent.is_none());
        if !root
            && !img
                .side_parent
                .is_some_and(|(_, _, p)| installed.contains(&p))
        {
            continue;
        }
        if let Some(mut a) = install(cs, req, img, todo, i == 0) {
            a.ahead = ahead;
            installed.push(img.id);
            out.push(a);
        } else if i == 0 {
            return Vec::new();
        }
    }
    out
}
