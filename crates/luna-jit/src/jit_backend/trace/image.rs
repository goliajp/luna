//! A compiled trace as other Vms take it over: its machine code, where it
//! holds Vm-specific addresses and what they stand for, and the dispatcher's
//! data beside it, none pointing into the Vm that compiled it.

use super::reloc::Code;
use super::*;
use luna_core::__private::TracePlans;
use luna_core::runtime::Value;

pub(crate) use luna_core::__private::{ProtoContent as Content, hash64};

mod meta;
pub(crate) use meta::Meta;

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
    /// The cell holding the trace's optimizing tier's loop.
    TierLoop,
    /// The trace's first side cell.
    SideCells,
    /// The Vm's run words.
    RunWords,
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
    /// Some of `sources` is [`Source::TierLoop`].
    pub(crate) at_loop: bool,
    /// Some of `sources` is [`Source::SideCells`].
    pub(crate) holds_cells: bool,
    pub(crate) meta: Meta,
    /// `(parent head pc, parent exit, parent image)` of a side trace.
    pub(crate) side_parent: Option<(u32, usize, u64)>,
    /// `None` for a trace nothing can enter, which keeps no code.
    pub(crate) code: Option<(Tier, Code)>,
    /// The optimizing tier's code, each for the calling exits it was
    /// compiled for, once some Vm compiled it (see [`TraceImage::optimized`]).
    optimized: std::sync::Mutex<Vec<std::sync::Arc<Optimized>>>,
    /// The baseline code made with its calling exits' calls, once some Vm
    /// made it (see `super::calls`): its relocations are those of
    /// `sources`, then the trace's first side cell.
    with_calls: std::sync::OnceLock<Code>,
    /// The baseline code's instructions, for a Vm that moves the trace to
    /// the optimizing tier or makes its calls before any other did.
    pub(crate) lir: Option<std::sync::Arc<lir::Lir>>,
    /// Bytes this image takes, roughly.
    pub(crate) size: usize,
}

/// The optimizing tier's code of a trace, compiled for the calling exits
/// `calls`: the entry, for a trace whose back edge calls the optimizing
/// tier's loop its first pass, and that loop.
pub(crate) struct Optimized {
    pub(crate) calls: lir::Calls,
    pub(crate) entry: Code,
    pub(crate) looped: Option<Code>,
}

/// The most codes of the optimizing tier an image keeps, each for other
/// calling exits.
const OPTIMIZED_KEPT: usize = 8;

impl TraceImage {
    /// The optimizing tier's code for the calling exits `calls`, once some
    /// Vm compiled it.
    pub(crate) fn optimized(&self, calls: &lir::Calls) -> Option<std::sync::Arc<Optimized>> {
        let kept = self.optimized.lock().expect(POISON);
        kept.iter().find(|o| o.calls == *calls).cloned()
    }

    /// [`TraceImage::optimized`] for `cells` calling exits none of which
    /// calls: what a Vm taking the image up runs, its cells not filled yet.
    pub(crate) fn optimized_unfilled(&self, cells: usize) -> Option<std::sync::Arc<Optimized>> {
        let kept = self.optimized.lock().expect(POISON);
        let unfilled = |c: &lir::Calls| match c {
            lir::Calls::Held(h) => h.len() == cells && h.iter().all(Option::is_none),
            _ => false,
        };
        kept.iter().find(|o| unfilled(&o.calls)).cloned()
    }

    /// Keeps `o` for other Vms to take up, unless the image keeps code for
    /// its calling exits already or as many codes as it may: whether it
    /// kept it.
    pub(crate) fn keep_optimized(&self, o: Optimized) -> bool {
        let mut kept = self.optimized.lock().expect(POISON);
        if kept.len() >= OPTIMIZED_KEPT || kept.iter().any(|k| k.calls == o.calls) {
            return false;
        }
        kept.push(std::sync::Arc::new(o));
        true
    }
}

impl TraceImage {
    /// The baseline code with its calls, once some Vm made it.
    pub(crate) fn with_calls(&self) -> Option<&Code> {
        self.with_calls.get()
    }

    /// Keeps `c`, the baseline code with its calls, for other Vms to take
    /// up, unless some Vm kept it already: whether it kept it.
    pub(crate) fn keep_with_calls(&self, c: Code) -> bool {
        self.with_calls.set(c).is_ok()
    }
}

const POISON: &str = "nothing panics holding an image's codes";

/// What compiling a shared trace hands [`build`]: its code, if any, and its relocations' values.
pub(crate) struct Captured {
    pub(crate) code: Option<(Tier, Code)>,
    pub(crate) relocs: Vec<(RelocKind, i64)>,
    pub(crate) lir: Option<std::sync::Arc<lir::Lir>>,
}

/// The image of `ct`, compiled from `record`; `None` when one of its
/// addresses means nothing another Vm can find.
pub(crate) fn build(
    record: &TraceRecord,
    ct: &CompiledTraceParts,
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
            RelocKind::TierLoop => Source::TierLoop,
            RelocKind::SideCells => Source::SideCells,
            RelocKind::RunWords => Source::RunWords,
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
        at_loop: sources.contains(&Source::TierLoop),
        holds_cells: sources.contains(&Source::SideCells),
        sources: sources.into(),
        meta,
        side_parent,
        code: cap.code,
        optimized: std::sync::Mutex::default(),
        with_calls: std::sync::OnceLock::new(),
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
