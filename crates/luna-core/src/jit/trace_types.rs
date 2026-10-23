//! Trace-JIT type definitions and small helpers shared across the
//! luna-core / luna split boundary.
//!
//! Everything here is cranelift-free by construction — these items
//! live in `luna-core`, while `trace.rs` (the codegen pipeline) lives
//! in `luna`. `jit::trace` re-exports this module so
//! `crate::jit::trace::*` paths remain compatible.

use crate::jit::send_compat::{TArc, TCellBool, TCellPtr, TCellU32, TRefLock};
use crate::runtime::Gc;
use crate::runtime::function::Proto;
use crate::vm::isa::Inst;

pub(crate) mod adopt;
pub(crate) mod compiled;
mod compiled_aot;
mod exit;
pub(crate) mod options;
pub(crate) mod record;
pub(crate) mod self_link;
pub(crate) mod side_exit;
pub(crate) use adopt::{AdoptRequest, AdoptedTrace};
pub(crate) use compiled::CompiledTrace;
pub use compiled::{
    CompileOutcome, INLINE_CLOSURE, INLINE_MULTI_RESULTS, INLINE_VAR_ARGS, INLINE_VARARG_CALLEE,
    TraceFn,
};
pub use exit::*;
pub(crate) use options::CompileOptions;
pub use record::{
    FIELD_SLOT_UNKNOWN, INDEX_SLOTS_UNKNOWN, RESULT_TAG_UNKNOWN, RecordOutcome, field_ic_enabled,
};
pub(crate) use record::{FieldIcSnapshot, RecordedOp, TraceRecord};
pub use self_link::SelfRecKind;
pub(crate) use self_link::{DownRecClose, RetfRecord};
pub use side_exit::{
    DOWNREC_MULTI_WAY_GUARD_MAX, HOTEXIT_THRESHOLD, SIDE_SENT_DOWNREC_CODE, SIDE_SENT_KIND_DOWNREC,
    SIDE_SENT_KIND_GLOBAL, SIDE_SENT_KIND_INLINE, SIDE_SENT_KIND_TAG, encode_side_sentinel,
    is_downrec_sentinel, v2c_probe_enabled,
};
pub(crate) use side_exit::{FrameMaterializeInfo, HotExitInfo, InlineSideExit};

/// Back-edge visit count after which a PC is promoted to a trace
/// head and recording begins. Tuned for benches in the 1k–10k
/// iteration range — too low and we record short traces that don't
/// pay back compile cost; too high and we never trace at all.
pub const TRACE_HOT_THRESHOLD: u32 = 64;

/// Call visit count after which a Proto is promoted to a
/// trace head at `pc=0` and recording begins. Separate from
/// [`TRACE_HOT_THRESHOLD`] so we can tune them independently — a
/// self-recursive function reaches its threshold via call counter
/// while its body's back-edges (if any) reach theirs via the
/// back-edge counter. Same value for now.
pub const CALL_HOT_THRESHOLD: u32 = 64;

/// Cap on the number of bytecode instructions captured in one trace.
/// Beyond this, recording aborts (the trace is too long to compile
/// usefully). PUC LuaJIT's default is 1024; luna starts conservative.
pub const MAX_TRACE_LEN: usize = 256;

/// `CompiledTrace::entry_tags` of a head-frame register the trace does not
/// read before writing it: not checked on entry, and an exit that has not
/// written it leaves the stack slot alone. No value tag uses this byte.
pub const ENTRY_TAG_ANY: u8 = 0xFF;

/// Max inline depth for self-recursive `Op::Call` during recording.
/// Beyond this, the trace emits a real cranelift `call` to itself.
pub const MAX_INLINE_DEPTH: u8 = 16;

/// Recunroll threshold (mirrors LuaJIT `lj_jit.h:123` default
/// `recunroll=2`). The recorder counts how many ancestor frames share
/// the trace head's proto; when the count EXCEEDS this threshold AND
/// we're about to execute the head_pc on the head_proto, close the
/// trace with `TraceEnd::SelfLink`. Default 2 = inline 2 recursion
/// levels (so the recorded body covers 3-deep fib body per loop iter
/// after the lowerer's bump-base + branch-to-self tail).
pub const RECUNROLL_THRESHOLD: usize = 2;

/// Whether a register holding a value of `tag` (a `raw` tag) can enter a
/// trace that reads it: a tag whose payload stands for the value, or a
/// boolean (the trace takes either value under `raw::FALSE`, with payload
/// 0 or 1; see [`entry_tag_of`]).
pub fn entry_tag_enterable(tag: u8) -> bool {
    (PLAIN_ENTRY_TAGS | 1 << crate::runtime::value::raw::FALSE) >> tag & 1 != 0
}

/// The tags [`entry_tag_enterable`] admits but `raw::FALSE`, a boolean's
/// entry type: a register of one of these enters a trace compiled for its
/// own tag with its payload as it is.
pub(crate) const PLAIN_ENTRY_TAGS: u32 = {
    use crate::runtime::value::raw;
    1 << raw::TRUE
        | 1 << raw::INT
        | 1 << raw::FLOAT
        | 1 << raw::TABLE
        | 1 << raw::CLOSURE
        | 1 << raw::NATIVE
        | 1 << raw::STR
        | 1 << raw::NIL
};

/// The entry tag a trace is compiled for when the register held a value of
/// raw tag `tag` while it was recorded: a boolean of either value is one
/// entry type.
pub fn entry_tag_of(tag: u8) -> u8 {
    use crate::runtime::value::raw;
    if tag == raw::TRUE { raw::FALSE } else { tag }
}

/// Default for [`CompileOptions::tier_up_at`].
pub const TIER_UP_THRESHOLD: u32 = 16384;

/// A trace whose function has been called again since the trace was
/// compiled moves to the optimizing tier after this fraction of
/// [`CompileOptions::tier_up_at`]: like HotSpot's tiered policy, which
/// weighs invocations as well as back edges, it tells code that keeps
/// being reused from a long loop that runs once.
pub const TIER_UP_REUSED_DIVISOR: u32 = 4;

/// The code generator a trace is compiled with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TraceTier {
    /// The baseline tier first, the optimizing one once the trace is hot.
    #[default]
    Auto,
    /// Only the baseline code generator (the optimizing one still takes a
    /// trace the baseline cannot handle).
    Baseline,
    /// Only the optimizing code generator (Cranelift).
    Optimizing,
}
