//! Traces another `Vm` compiled, taken over without compiling them again
//! (see [`crate::jit::TraceCompiler::adopt_traces`]).

use super::*;

/// Where the interpreter would start recording a trace, asked of the
/// backend first: a trace compiled for code of the same content may be
/// installed instead.
#[doc(hidden)]
pub struct AdoptRequest<'a> {
    /// The function the trace would start in.
    pub proto: Gc<Proto>,
    pub head_pc: u32,
    /// The raw tags of the frame's registers now.
    pub entry_tags: &'a [u8],
    /// For a side trace: the head pc of the trace whose exit became hot,
    /// and that exit's index (as [`TraceRecord::side_trace_parent`]).
    pub side_parent: Option<(u32, usize)>,
    /// Recording would start at a call, rather than at a loop.
    pub call_triggered: bool,
    /// As [`TraceRecord::settings`] for a recording now.
    pub settings: u8,
    /// The options the trace would be compiled with.
    pub opts: CompileOptions,
    pub version: crate::version::LuaVersion,
    /// The functions of every chunk this `Vm` loaded that is still alive,
    /// by their main functions.
    pub roots: &'a [Gc<Proto>],
    /// The metamethod names, by `TM` index.
    pub mm_names: &'a [Gc<crate::runtime::LuaStr>],
}

/// A trace the backend installed for an [`AdoptRequest`].
#[doc(hidden)]
pub struct AdoptedTrace {
    /// As the backend would return it right after compiling it.
    pub trace: CompiledTrace,
    /// For a side trace: as [`AdoptRequest::side_parent`].
    pub side_parent: Option<(u32, usize)>,
    /// The functions other than the head's that the trace inlined.
    pub inlined: Vec<Gc<Proto>>,
}

/// Whether a trace compiled for `compiled` entry tags admits registers whose
/// tags are `now`, by the dispatcher's rule.
#[doc(hidden)]
pub fn entry_tags_admit(compiled: &[u8], now: &[u8]) -> bool {
    use crate::runtime::value::raw;
    compiled.iter().enumerate().all(|(i, &want)| {
        if want == ENTRY_TAG_ANY {
            return true;
        }
        let Some(&tag) = now.get(i) else {
            return false;
        };
        if tag == want {
            entry_tag_enterable(tag)
        } else {
            want == raw::FALSE && tag == raw::TRUE
        }
    })
}
