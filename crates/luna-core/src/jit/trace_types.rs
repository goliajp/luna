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

mod adopt;
mod compiled;
mod compiled_aot;
mod exit;
mod record;
mod self_link;
mod side_exit;
pub use adopt::*;
pub use compiled::*;
pub use exit::*;
pub use record::*;
pub use self_link::*;
pub use side_exit::*;

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

/// Compile-time options for the trace lowerer.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    /// When `true`, the trace's clean-close path emits a back-edge
    /// jump to its own body-loop block instead of returning
    /// `head_pc` to the caller — so the JIT'd code runs in a tight
    /// native loop until a cmp side-exit fires. The dispatcher's
    /// per-entry marshal cost amortizes across however many
    /// iterations the trace runs before diverging.
    ///
    /// Internal-loop traces require at least one exit edge
    /// (`Lt / Le / Eq` cmp or `Op::ForLoop`) AND no `Op::Call`
    /// truncation — otherwise the trace would run forever. The
    /// lowerer auto-downgrades to one-shot when neither condition
    /// holds, so callers can safely set this `true` for any
    /// record.
    ///
    /// Defaults to `false` in `try_compile_trace` (one-shot, the
    /// shape unit tests assume) and `true` in
    /// `try_compile_trace_with_options` when callers explicitly
    /// want the dispatcher fast path.
    pub internal_loop: bool,
    /// Lua dialect — `true` for 5.1 / 5.2 / 5.3, `false` for
    /// 5.4 / 5.5. The numeric `for` op (`Op::ForLoop`) has a
    /// different layout pre-5.3 (the slot at `R[A+1]` is the raw
    /// `limit` Value, not a remaining-iteration count). Only the
    /// 5.4+ Int count form is lowered; pre-5.3 traces bail
    /// and stay on the interp side.
    pub pre53: bool,
    /// Emit AOT-relocatable IR.
    ///
    /// When `false` (the JIT default), interned-string-key arguments
    /// to `luna_jit_*_field` helpers are baked as
    /// `iconst(I64, key_str.as_ptr() as i64)` — the live runtime
    /// pointer, valid because the lowered mcode runs in the same
    /// process / `Vm` / `StringTable` as the recorder.
    ///
    /// When `true` (the AOT path), the lowerer instead routes each
    /// key through a writable data slot named
    /// `__luna_aot_strkey_slot_<hex>` (8 bytes, zero-initialised at
    /// link time) and emits a load through that symbol. A sibling
    /// read-only object `__luna_aot_strkey_bytes_<hex>` carries the
    /// UTF-8 bytes; the deploy-side startup hook walks every
    /// `_bytes_<hex>` symbol, interns the bytes into the deploy
    /// `Vm`'s `StringTable`, and writes the resulting
    /// `Gc<LuaStr>::as_ptr()` into the matching `_slot_<hex>` before
    /// any AOT trace dispatches. JIT path is unaffected — same
    /// `iconst` it always emitted.
    pub aot: bool,
    /// Which code generator compiles the trace.
    pub tier: TraceTier,
    /// With [`TraceTier::Auto`]: the loop iterations plus entries after
    /// which the trace is compiled again by the optimizing tier (`0`:
    /// never).
    pub tier_up_at: u32,
    /// The dialect of the Vm the trace runs in, when known: inline table
    /// code then follows only that dialect's length rules (5.4 keeps a
    /// length limit array indexing can move, 5.5 a length hint). `None`
    /// compiles code that serves every dialect.
    pub dialect: Option<crate::version::LuaVersion>,
}

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

/// Entries of a trace between two asks of a backend still compiling
/// better code for it in the background (see `TraceCompiler::tier_up`):
/// asking at every entry costs a trace entered often more than it gains.
pub const TIER_UP_REASK: u32 = 64;

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
