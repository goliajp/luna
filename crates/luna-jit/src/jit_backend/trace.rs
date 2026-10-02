//! Trace JIT data structures and lowering.
//!
//! Where `src/jit/mod.rs` is the *method* JIT (compiles one Proto's
//! body to cranelift IR), this module is the *trace* JIT: it records
//! one linear bytecode path through a hot back-edge, including
//! cross-call inlining, and compiles the recorded trace as a single
//! cranelift function with side-exit guards.
//!
//! The split lets the two co-exist: small leaf functions stay on the
//! method JIT (zero startup cost, no recording), while hot loops and
//! recursive functions move to the trace JIT once the per-Proto hot
//! counter passes [`TRACE_HOT_THRESHOLD`].

use luna_core::jit::send_compat::{TArc, TCellBool, TCellPtr, TCellU32, TRefLock};
use luna_core::runtime::Gc;
use luna_core::runtime::function::Proto;
use luna_core::vm::isa::{Inst, Op};

// Pure data types live in `luna_core::jit::trace_types`. Re-export so existing
// `crate::jit_backend::trace::TraceRecord` paths within luna continue
// to resolve, and so the v1.0-compatible `luna_jit::jit::trace::*` surface
// (assembled in `luna_jit::lib::jit::trace`) sees both type defs and
// codegen entry points side-by-side.
pub use luna_core::jit::trace_types::*;

mod const_operands;
mod math_fold;
mod op_helpers;
use math_fold::*;
use op_helpers::*;
mod entry;
mod field_slot;
mod kinds;
mod slots;
use const_operands::{VConst, split_const_operands};
use cranelift::prelude::*;
use cranelift_codegen::ir::UserFuncName;
use cranelift_codegen::settings;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use entry::{entry_live, side_parent_exit_tags};
use kinds::*;
use slots::op_writes_at_offset;
pub use slots::{compute_body_writes, compute_live_in_slots, op_reads_writes};
mod accum;
mod aot_data;
mod block_params;
mod compile;
mod escape;
mod escape_scan;
mod escape_sweep;
mod exits;
mod lower;
mod shape;
mod sunk;
mod value_ops;
use accum::*;
use aot_data::*;
use block_params::*;
pub use compile::*;
pub use escape::*;
use escape_scan::*;
use escape_sweep::*;
use exits::*;
use lower::*;
use shape::*;
use sunk::*;
pub(super) use value_ops::emit_f64_fits_i64;
use value_ops::*;

// `pub enum TagResKind` + `pub(crate) fn classify_exit_tags` moved to
// `trace_types.rs`; re-exported via `pub use super::trace_types::*;`.

#[cfg(test)]
mod tests;
