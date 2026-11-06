//! The method JIT and compiled traces on the interpreter's side: entering
//! compiled code, the levels of its self calls, the frames it hands back,
//! and the runtime calls it makes.

use super::*;

pub(in crate::vm::exec) mod call;
pub(crate) mod handoff;
pub(crate) mod levels;
pub(in crate::vm::exec) mod rt;
pub(in crate::vm::exec) mod tforcall;
