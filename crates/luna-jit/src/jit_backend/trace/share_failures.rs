//! Recordings that failed to compile, shared between the Vms of one
//! [`crate::Engine`]. A failure is kept with its fingerprint: the head's
//! code (by content), how the recording started (at a loop, at a call, or
//! at which exit of which parent), the entry tags, and the path the
//! recording took. Another Vm whose recording has the same fingerprint
//! does not compile it, and counts the failures as its own: having failed
//! as often as the first Vm did, it gives the head up without recording it
//! again. A recording with another fingerprint compiles as usual.

use super::image::{Content, Settings, hash64};
use super::*;
use crate::jit_backend::engine::HeadKey;

/// One recording that failed to compile.
pub(crate) struct Failure {
    head: Content,
    entry_tags: Box<[u8]>,
    /// For a side trace: the parent's head pc and exit.
    side: Option<(u32, usize)>,
    /// What the recording ran: [`path_of`].
    path: u64,
    /// The recordings that failed this way.
    times: u32,
    /// The Vm that had it (`CraneliftJitStorage::engine_vm`): its own
    /// failures it counted already.
    vm: u64,
}

/// Failures kept for one head, at most.
const PER_HEAD: usize = 8;

/// The fingerprint of the path a recording took: each op's function (by
/// content), pc, inline depth and instruction.
fn path_of(record: &TraceRecord) -> u64 {
    let mut seen: Vec<(*const Proto, u64)> = Vec::new();
    let mut bytes = Vec::with_capacity(record.ops.len() * 24);
    for op in &record.ops {
        let p = op.proto.as_ptr() as *const Proto;
        let h = match seen.iter().find(|(q, _)| *q == p) {
            Some(&(_, h)) => h,
            None => {
                let h = Content::of(&op.proto).hash;
                seen.push((p, h));
                h
            }
        };
        bytes.extend_from_slice(&h.to_le_bytes());
        bytes.extend_from_slice(&op.pc.to_le_bytes());
        bytes.extend_from_slice(&op.inst.0.to_le_bytes());
        bytes.push(op.inline_depth);
        bytes.extend_from_slice(&op.var_count.unwrap_or(u32::MAX).to_le_bytes());
    }
    hash64(&bytes)
}

fn key_of(settings: Settings, head: &Content, head_pc: u32) -> HeadKey {
    HeadKey {
        settings,
        content: head.hash,
        head_pc,
    }
}

/// The engine of `storage`'s Vm and the key and content of `record`'s
/// head, when that Vm has an engine.
fn record_key(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    version: luna_core::version::LuaVersion,
) -> Option<(crate::Engine, u64, HeadKey, Content)> {
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    let (engine, vm) = (cs.engine.clone()?, cs.engine_vm);
    let head = Content::of(&record.head_proto);
    let settings = Settings::new(version, opts, record.is_call_triggered, record.settings);
    let key = key_of(settings, &head, record.head_pc);
    Some((engine, vm, key, head))
}

fn side_of(record: &TraceRecord) -> Option<(u32, usize)> {
    record.side_trace_parent.map(|(_, pc, exit)| (pc, exit))
}

/// [`luna_core::jit::TraceCompiler::failure_known`].
pub(crate) fn failure_known(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    version: luna_core::version::LuaVersion,
) -> u32 {
    let Some((engine, vm, key, head)) = record_key(storage, record, opts, version) else {
        return 0;
    };
    if !engine.cache().failures.contains_key(&key) {
        return 0;
    }
    let path = path_of(record);
    let side = side_of(record);
    let cache = engine.cache();
    cache.failures.get(&key).map_or(0, |list| {
        list.iter()
            .filter(|f| {
                f.vm != vm
                    && f.path == path
                    && f.head == head
                    && *f.entry_tags == *record.entry_tags
                    && f.side == side
            })
            .map(|f| f.times)
            .sum()
    })
}

/// [`luna_core::jit::TraceCompiler::publish_failure`].
pub(crate) fn publish_failure(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    version: luna_core::version::LuaVersion,
) {
    let Some((engine, vm, key, head)) = record_key(storage, record, opts, version) else {
        return;
    };
    let failure = Failure {
        head,
        entry_tags: record.entry_tags.clone().into(),
        side: side_of(record),
        path: path_of(record),
        times: 1,
        vm,
    };
    engine.cache().insert_failure(key, failure, PER_HEAD);
}

impl Failure {
    /// One more recording failed as `self` did.
    pub(crate) fn again(&mut self) {
        self.times = self.times.saturating_add(1);
    }

    /// Whether `self` and `other` are the same failure, in the same Vm.
    pub(crate) fn same(&self, other: &Failure) -> bool {
        self.vm == other.vm
            && self.path == other.path
            && self.side == other.side
            && self.entry_tags == other.entry_tags
            && self.head == other.head
    }

    /// Bytes held, roughly.
    pub(crate) fn size(&self) -> usize {
        std::mem::size_of::<Failure>() + self.head.bytes.len() + self.entry_tags.len()
    }
}
