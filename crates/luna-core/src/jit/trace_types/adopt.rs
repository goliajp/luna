//! Traces another `Vm` compiled, taken over without compiling them again
//! (see [`crate::jit::TraceCompiler::adopt_traces`]).

use super::*;

/// Where the interpreter would start recording a trace, asked of the
/// backend first: a trace compiled for code of the same content may be
/// installed instead.
pub struct AdoptRequest<'a> {
    /// The function the trace would start in.
    pub proto: Gc<Proto>,
    /// The pc the trace would start at.
    pub head_pc: u32,
    /// The raw tags of the frame's registers now.
    pub entry_tags: &'a [u8],
    /// For a side trace: the trace whose exit became hot, and that exit's
    /// index (as [`TraceRecord::side_trace_parent`]).
    pub side_parent: Option<(ParentId, usize)>,
    /// Recording would start at a call, rather than at a loop.
    pub call_triggered: bool,
    /// As [`TraceRecord::settings`] for a recording now.
    pub settings: u8,
    /// The options the trace would be compiled with.
    pub opts: CompileOptions,
    /// The dialect the Vm runs.
    pub version: crate::version::LuaVersion,
    /// The functions of every chunk this `Vm` loaded that is still alive,
    /// by their main functions.
    pub roots: &'a [Gc<Proto>],
    /// The metamethod names, by `TM` index.
    pub mm_names: &'a [Gc<crate::runtime::LuaStr>],
    /// Also install, as [`AdoptedTrace::ahead`], a trace for each other
    /// loop head of the function that has none yet, so that the code of all
    /// of them turns executable at once.
    pub ahead: bool,
}

/// A trace the backend installed for an [`AdoptRequest`].
pub struct AdoptedTrace {
    /// As the backend would return it right after compiling it, shared
    /// already, as the Vm caches it (no other holds it yet).
    pub trace: crate::jit::send_compat::TArc<CompiledTrace>,
    /// For a side trace: its parent and the parent's exit it runs from.
    pub side_parent: Option<(AdoptedParent, usize)>,
    /// The image the trace was taken up from, which a side trace taken up
    /// with it names as [`AdoptedParent::Image`].
    pub image: u64,
    /// The functions other than the head's that the trace inlined.
    pub inlined: Vec<Gc<Proto>>,
    /// The trace starts at a head other than [`AdoptRequest::head_pc`]
    /// (asked for by [`AdoptRequest::ahead`]), or is a side trace of one
    /// that does: it comes after its parent.
    pub ahead: bool,
}

/// The parent of a side trace taken up from another Vm.
#[derive(Clone, Copy, Debug)]
pub enum AdoptedParent {
    /// The trace [`AdoptRequest::side_parent`] names.
    Asked(ParentId),
    /// The trace taken up, before it in the same batch, from the image of
    /// this [`AdoptedTrace::image`].
    Image(u64),
}

/// Whether a trace compiled for `compiled` entry tags admits registers whose
/// tags are `now`, by the dispatcher's rule.
pub(crate) fn entry_tags_admit(compiled: &[u8], now: &[u8]) -> bool {
    entry_tags_admit_with(compiled, |i| now.get(i).copied())
}

/// [`entry_tags_admit`] for the tag of each register `now` gives.
pub(crate) fn entry_tags_admit_with(compiled: &[u8], now: impl Fn(usize) -> Option<u8>) -> bool {
    use crate::runtime::value::raw;
    compiled.iter().enumerate().all(|(i, &want)| {
        if want == ENTRY_TAG_ANY {
            return true;
        }
        let Some(tag) = now(i) else {
            return false;
        };
        if tag == want {
            entry_tag_enterable(tag)
        } else {
            want == raw::FALSE && tag == raw::TRUE
        }
    })
}

/// The traces a `Vm` took up for loop heads of one function before those
/// heads were reached: each with the side traces that came with it, its
/// parent first.
#[derive(Default)]
pub(crate) struct TraceAhead {
    /// The function's traces were asked for with [`AdoptRequest::ahead`]
    /// already, and it got some: it is not asked again.
    pub(crate) asked: bool,
    /// The traces of each head, the one starting there first.
    pub(crate) heads: Vec<Vec<AdoptedTrace>>,
}
