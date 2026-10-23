//! The types, modules and constants luna's other crates use that are not
//! part of the public API.

pub use crate::frontend::ast::rhs_calls::RhsCallScan;
pub use crate::frontend::lexer::source::{Source, Whole};
pub use crate::jit::abi::TraceCompiler;
pub use crate::jit::rebind::JitVmRebindRestore;
pub use crate::jit::storage::JitStorage;
pub use crate::jit::trace_types::adopt::{AdoptRequest, AdoptedTrace};
pub use crate::jit::trace_types::compiled::{CompiledTrace, TierUp};
pub use crate::jit::trace_types::options::CompileOptions;
pub use crate::jit::trace_types::record::{FieldIcSnapshot, RecordedOp, TraceRecord};
pub use crate::jit::trace_types::self_link::{DownRecClose, RetfRecord};
pub use crate::jit::trace_types::side_exit::{FrameMaterializeInfo, HotExitInfo, InlineSideExit};
pub use crate::runtime::builtin::Builtin;
pub use crate::runtime::value::raw::RawVal;
pub use crate::stdio::host::HostStdout;
pub use crate::vm::exec::errors::SpecialErrors;
pub use crate::vm::exec::host_c::debug::{HostAr, HostHookFn, HostLevel};
pub use crate::vm::exec::host_c::libs::HostOpenError;
pub use crate::vm::exec::host_c::load::HostLoadError;
pub use crate::vm::exec::host_c::values::{HOST_OP_BNOT, HOST_OP_UNM};
pub use crate::vm::exec::jit_levels::JitLimits;
pub use crate::vm::exec::load::text::{ParsedText, TextLoad};
pub use crate::vm::jit_state::{JitCounters, JitState};

/// The tags of a value's array-part encoding.
pub mod raw {
    pub use crate::runtime::value::raw::*;
}

/// errno as the C library the dialect's PUC build links sets it.
pub mod cerrno {
    pub use crate::cerrno::*;
}

/// The running thread's stack bounds.
pub mod native_stack {
    pub use crate::native_stack::*;
}
