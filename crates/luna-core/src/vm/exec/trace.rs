//! The trace JIT on the interpreter's side: recording, entering a compiled
//! trace, its exits and side traces, and what failed traces leave behind.

use super::*;

pub(in crate::vm::exec) mod adopt;
pub(in crate::vm::exec) mod cache;
pub(in crate::vm::exec) mod close;
pub(in crate::vm::exec) mod dispatch;
pub(in crate::vm::exec) mod exit;
pub(in crate::vm::exec) mod fail_memory;
pub(in crate::vm::exec) mod gc_quiet;
pub(in crate::vm::exec) mod record;
pub(in crate::vm::exec) mod restore;
pub(in crate::vm::exec) mod start;
pub(in crate::vm::exec) mod stats;
pub(in crate::vm::exec) mod step;
pub(in crate::vm::exec) mod wire;
