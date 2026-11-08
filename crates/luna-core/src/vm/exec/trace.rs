//! The trace JIT on the interpreter's side: recording, entering a compiled
//! trace, its exits and side traces, and what failed traces leave behind.

use super::*;

pub(in crate::vm::exec) mod adopt;
mod ahead;
pub(in crate::vm::exec) mod cache;
pub(in crate::vm::exec) mod close;
mod counting;
pub(in crate::vm::exec) mod dispatch;
mod downrec;
pub(in crate::vm::exec) mod exit;
pub(in crate::vm::exec) mod fail_memory;
pub(in crate::vm::exec) mod gc_quiet;
mod link;
pub(in crate::vm::exec) mod record;
pub(in crate::vm::exec) mod relink;
pub(crate) mod restore;
pub(in crate::vm::exec) mod start;
pub(in crate::vm::exec) mod stats;
pub(in crate::vm::exec) mod step;
pub(in crate::vm::exec) mod wire;
