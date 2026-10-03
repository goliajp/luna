//! A compiled trace as other Vms take it over: its machine code with the
//! places that hold Vm-specific addresses, what those addresses stand for,
//! and the data the dispatcher keeps beside the code. Nothing in it points
//! into the Vm that compiled it.

use super::reloc::Code;
use super::*;
use luna_core::jit::trace_types::{
    CompiledTrace, ExitTag, FrameMaterializeInfo, InlineSideExit, TagResKind, TierUp,
};
use luna_core::runtime::Value;

/// The content of a function prototype a trace depends on: what the
/// lowering reads of it and of the prototypes it creates closures of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Content {
    pub(crate) bytes: Box<[u8]>,
    pub(crate) hash: u64,
}

impl Content {
    pub(crate) fn of(p: &Proto) -> Content {
        let mut out = Vec::with_capacity(p.code.len() * 4 + 64);
        write_content(p, &mut out);
        let hash = hash64(&out);
        Content {
            bytes: out.into(),
            hash,
        }
    }

    /// Whether `p` has this content.
    pub(crate) fn matches(&self, p: &Proto) -> bool {
        // the cheap fields first: most prototypes differ in them
        if p.code.len() * 4 > self.bytes.len() || p.max_stack != self.bytes[self.bytes.len() - 1] {
            return false;
        }
        let mut out = Vec::with_capacity(self.bytes.len());
        write_content(p, &mut out);
        *out == *self.bytes
    }
}

fn write_content(p: &Proto, out: &mut Vec<u8>) {
    for i in p.code.iter() {
        out.extend_from_slice(&i.0.to_le_bytes());
    }
    out.extend_from_slice(&(p.consts.len() as u32).to_le_bytes());
    for c in p.consts.iter() {
        match c {
            Value::Nil => out.push(0),
            Value::Bool(b) => out.extend_from_slice(&[1, u8::from(*b)]),
            Value::Int(i) => {
                out.push(2);
                out.extend_from_slice(&i.to_le_bytes());
            }
            Value::Float(f) => {
                out.push(3);
                out.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            Value::Str(s) => {
                out.push(4);
                out.extend_from_slice(&(s.as_bytes().len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            // the compiler makes no other constants
            _ => out.push(255),
        }
    }
    write_upvals(p, out);
    // what creating a closure of a nested function reads of it
    out.extend_from_slice(&(p.protos.len() as u32).to_le_bytes());
    for c in p.protos.iter() {
        write_upvals(c, out);
        out.extend_from_slice(&[c.num_params, u8::from(c.is_vararg), c.max_stack]);
    }
    out.extend_from_slice(&[
        u8::from(p.has_vararg_table_pseudo),
        u8::from(p.has_compat_vararg_arg),
        p.env_upval_idx,
        p.num_params,
        u8::from(p.is_vararg),
        p.max_stack,
    ]);
}

fn write_upvals(p: &Proto, out: &mut Vec<u8>) {
    out.extend_from_slice(&(p.upvals.len() as u32).to_le_bytes());
    for u in p.upvals.iter() {
        let name = u.name.as_bytes();
        out.extend_from_slice(&[u8::from(u.in_stack), u.index, u8::from(u.read_only)]);
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name);
    }
}

/// A 64-bit hash of `b`, eight bytes at a time. It only picks the bucket:
/// a hit is confirmed by comparing the content itself.
pub(crate) fn hash64(b: &[u8]) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15 ^ b.len() as u64;
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().expect("eight bytes"));
        h = (h.rotate_left(5) ^ w).wrapping_mul(0x51_7cc1_b727_220a_95);
    }
    for &x in chunks.remainder() {
        h = (h.rotate_left(5) ^ u64::from(x)).wrapping_mul(0x51_7cc1_b727_220a_95);
    }
    h ^ (h >> 29)
}

/// What a relocated address stands for, in terms another Vm can resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Constant `k` (a string) of prototype `p` of [`TraceImage::protos`].
    Const { p: u32, k: u32 },
    /// The metamethod name with these bytes.
    MetaName(Box<[u8]>),
    /// Prototype `p` of [`TraceImage::protos`].
    Proto(u32),
    /// The frame chain of inline side exit `n`.
    Chain(u32),
    /// The trace's iteration count for moving to the optimizing tier.
    TierCell,
}

/// The settings a trace was recorded and compiled under: a trace is shared
/// only between Vms where all of them agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Settings {
    pub(crate) version: u8,
    pub(crate) internal_loop: bool,
    pub(crate) pre53: bool,
    pub(crate) tier: u8,
    pub(crate) tier_up_at: u32,
    pub(crate) call_triggered: bool,
    pub(crate) recording: u8,
}

impl Settings {
    pub(crate) fn new(
        version: luna_core::version::LuaVersion,
        opts: CompileOptions,
        call_triggered: bool,
        recording: u8,
    ) -> Settings {
        Settings {
            version: version as u8,
            internal_loop: opts.internal_loop,
            pre53: opts.pre53,
            tier: opts.tier as u8,
            tier_up_at: opts.tier_up_at,
            call_triggered,
            recording,
        }
    }
}

/// Which code generator produced [`TraceImage::code`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    Baseline,
    Optimizing,
}

/// One compiled trace, shareable between Vms.
pub(crate) struct TraceImage {
    pub(crate) id: u64,
    /// The head's prototype first, then those of the functions it inlined
    /// and whose constants it reads.
    pub(crate) protos: Box<[Content]>,
    /// One per relocation index of the code.
    pub(crate) sources: Box<[Source]>,
    pub(crate) meta: Meta,
    /// `(parent head pc, parent exit, parent image)` of a side trace.
    pub(crate) side_parent: Option<(u32, usize, u64)>,
    /// `None` for a trace nothing can enter, which keeps no code.
    pub(crate) code: Option<(Tier, Code)>,
    /// The optimizing tier's code, once some Vm compiled it.
    pub(crate) optimized: std::sync::OnceLock<Code>,
    /// The baseline code's instructions, for a Vm that moves the trace to
    /// the optimizing tier before any other did.
    pub(crate) lir: Option<std::sync::Arc<lir::Lir>>,
    /// Bytes this image takes, roughly.
    pub(crate) size: usize,
}

/// The data of a [`CompiledTrace`] beside its code, without the cells a
/// running trace updates.
pub(crate) struct Meta {
    head_pc: u32,
    n_ops: u32,
    dispatchable: bool,
    window_size: u32,
    exit_tags: Box<[ExitTag]>,
    global_tag_res_kind: TagResKind,
    pub(crate) entry_tags: Box<[u8]>,
    per_exit_tags: Box<[(u32, Box<[ExitTag]>)]>,
    per_exit_inline: Box<[(u32, u32, Box<[ExitTag]>, Box<[FrameMaterializeInfo]>)]>,
    is_inline_abort_close: bool,
    dispatch_off_reason: Option<&'static str>,
    sinkable_sites_seen: u32,
    accum_bufferable_seen: u32,
    sunk_alloc_seen: u32,
    materialize_emit_count: u32,
    closure_seen: u32,
    body_writes: Box<[u32]>,
    downrec_link: Option<(u32, u32)>,
    downrec_multi_way_count: u8,
    pub(crate) tier_at: Option<u32>,
}

impl Meta {
    pub(crate) fn of(ct: &CompiledTrace) -> Meta {
        Meta {
            head_pc: ct.head_pc,
            n_ops: ct.n_ops,
            dispatchable: ct.dispatchable,
            window_size: ct.window_size,
            exit_tags: ct.exit_tags.iter().copied().collect(),
            global_tag_res_kind: ct.global_tag_res_kind,
            entry_tags: ct.entry_tags.iter().copied().collect(),
            per_exit_tags: ct
                .per_exit_tags
                .iter()
                .map(|(pc, t)| (*pc, t.iter().copied().collect()))
                .collect(),
            per_exit_inline: ct
                .per_exit_inline
                .iter()
                .map(|e| {
                    (
                        e.cont_pc,
                        e.head_resume_pc,
                        e.exit_tags.iter().copied().collect(),
                        e.chain.iter().copied().collect(),
                    )
                })
                .collect(),
            is_inline_abort_close: ct.is_inline_abort_close,
            dispatch_off_reason: ct.dispatch_off_reason,
            sinkable_sites_seen: ct.sinkable_sites_seen,
            accum_bufferable_seen: ct.accum_bufferable_seen,
            sunk_alloc_seen: ct.sunk_alloc_seen,
            materialize_emit_count: ct.materialize_emit_count,
            closure_seen: ct.closure_seen,
            body_writes: ct.body_writes.clone(),
            downrec_link: ct.downrec_link,
            downrec_multi_way_count: ct.downrec_multi_way_count,
            tier_at: ct.tier_up.as_ref().map(|t| t.at),
        }
    }

    /// A fresh [`CompiledTrace`] (no code yet: `entry` is the placeholder),
    /// with its own cells. `tiered`: the trace runs the optimizing tier's
    /// code and moves no further.
    pub(crate) fn instantiate(&self, tiered: bool, calls_at: u32) -> CompiledTrace {
        let arc =
            |t: &[ExitTag]| -> TArc<[ExitTag]> { t.iter().copied().collect::<Vec<_>>().into() };
        let per_exit_tags: Vec<(u32, TArc<[ExitTag]>)> = self
            .per_exit_tags
            .iter()
            .map(|(pc, t)| (*pc, arc(t)))
            .collect();
        let per_exit_inline: Vec<InlineSideExit> = self
            .per_exit_inline
            .iter()
            .map(|(cont_pc, head_resume_pc, tags, chain)| InlineSideExit {
                cont_pc: *cont_pc,
                head_resume_pc: *head_resume_pc,
                exit_tags: arc(tags),
                chain: chain.iter().copied().collect::<Vec<_>>().into(),
                side_trace_ptr: Box::new(TCellPtr::null()),
            })
            .collect();
        let total = per_exit_inline.len() + per_exit_tags.len() + 1;
        let tier_up = match self.tier_at {
            Some(at) if !tiered => Some(Box::new(TierUp {
                count: Box::new(TCellU32::new(0)),
                at,
                calls_at,
                optimized: TCellPtr::null(),
                tried: TCellBool::new(false),
                parent_cells: [TCellPtr::null(), TCellPtr::null()],
                source: TRefLock::new(None),
            })),
            _ => None,
        };
        CompiledTrace {
            head_pc: self.head_pc,
            entry: placeholder_trace_fn,
            n_ops: self.n_ops,
            dispatchable: self.dispatchable,
            window_size: self.window_size,
            exit_tags: arc(&self.exit_tags),
            global_tag_res_kind: self.global_tag_res_kind,
            entry_tags: self.entry_tags.iter().copied().collect::<Vec<_>>().into(),
            tags_side_trace_ptrs: (0..per_exit_tags.len())
                .map(|_| Box::new(TCellPtr::null()))
                .collect::<Vec<_>>()
                .into(),
            per_exit_tags: per_exit_tags.into(),
            per_exit_inline: per_exit_inline.into(),
            exit_hit_counts: (0..total)
                .map(|_| TCellU32::new(0))
                .collect::<Vec<_>>()
                .into(),
            exit_side_trace_ptrs: (0..total)
                .map(|_| TCellPtr::null())
                .collect::<Vec<_>>()
                .into(),
            global_side_trace_ptr: Box::new(TCellPtr::null()),
            side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
            has_any_side_wired: TCellBool::new(false),
            is_inline_abort_close: self.is_inline_abort_close,
            dispatch_off_reason: self.dispatch_off_reason,
            sinkable_sites_seen: self.sinkable_sites_seen,
            accum_bufferable_seen: self.accum_bufferable_seen,
            sunk_alloc_seen: self.sunk_alloc_seen,
            materialize_emit_count: self.materialize_emit_count,
            closure_seen: self.closure_seen,
            body_writes: self.body_writes.clone(),
            downrec_link: self.downrec_link,
            downrec_multi_way_count: self.downrec_multi_way_count,
            tier_up,
        }
    }

    fn size(&self) -> usize {
        std::mem::size_of::<Meta>()
            + self.exit_tags.len()
            + self.entry_tags.len()
            + self
                .per_exit_tags
                .iter()
                .map(|(_, t)| 8 + t.len())
                .sum::<usize>()
            + self
                .per_exit_inline
                .iter()
                .map(|(_, _, t, c)| 16 + t.len() + 12 * c.len())
                .sum::<usize>()
            + 4 * self.body_writes.len()
    }
}

/// What the compile of a trace for a Vm that shares its traces hands to
/// [`build`]: the code, if any, and what its relocations hold here.
pub(crate) struct Captured {
    pub(crate) code: Option<(Tier, Code)>,
    pub(crate) relocs: Vec<(RelocKind, i64)>,
    pub(crate) lir: Option<std::sync::Arc<lir::Lir>>,
}

/// The image of `ct`, compiled from `record`; `None` when one of its
/// addresses means nothing another Vm can find.
pub(crate) fn build(
    record: &TraceRecord,
    ct: &CompiledTrace,
    side_parent: Option<(u32, usize, u64)>,
    cap: Captured,
    id: u64,
) -> Option<TraceImage> {
    let mut protos: Vec<Gc<Proto>> = vec![record.head_proto];
    for op in &record.ops {
        if !protos.iter().any(|p| p.ptr_eq(op.proto)) {
            protos.push(op.proto);
        }
    }
    let mut sources = Vec::with_capacity(cap.relocs.len());
    for &(kind, live) in &cap.relocs {
        sources.push(match kind {
            RelocKind::Str => string_source(&protos, record, live)?,
            RelocKind::Proto => {
                let p = protos.iter().position(|p| p.as_ptr() as i64 == live)?;
                Source::Proto(p as u32)
            }
            RelocKind::Chain(n) => Source::Chain(n),
            RelocKind::TierCell => Source::TierCell,
        });
    }
    let meta = Meta::of(ct);
    let contents: Vec<Content> = protos.iter().map(|p| Content::of(p)).collect();
    let code_size = cap.code.as_ref().map_or(0, |(_, c)| c.bytes.len());
    let size = std::mem::size_of::<TraceImage>()
        + meta.size()
        + contents.iter().map(|c| c.bytes.len()).sum::<usize>()
        + code_size
        + cap.lir.as_ref().map_or(0, |l| l.size());
    Some(TraceImage {
        id,
        protos: contents.into(),
        sources: sources.into(),
        meta,
        side_parent,
        code: cap.code,
        optimized: std::sync::OnceLock::new(),
        lir: cap.lir,
        size,
    })
}

/// A string the code holds: a constant of one of `protos`, or the
/// metamethod name the recording took the method lookup through.
fn string_source(protos: &[Gc<Proto>], record: &TraceRecord, live: i64) -> Option<Source> {
    for (p, proto) in protos.iter().enumerate() {
        let k = proto
            .consts
            .iter()
            .position(|c| matches!(c, Value::Str(s) if s.as_ptr() as i64 == live));
        if let Some(k) = k {
            return Some(Source::Const {
                p: p as u32,
                k: k as u32,
            });
        }
    }
    let key = record.index_key.filter(|k| k.as_ptr() as i64 == live)?;
    Some(Source::MetaName(key.as_bytes().into()))
}
