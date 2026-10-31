//! The interpreter's stacks: the main stack, the part of a stack in use,
//! and how PUC sizes a stack as it grows and shrinks.

use super::*;

pub(in crate::vm::exec) mod inuse;
pub(in crate::vm::exec) mod main;
pub(crate) mod size;
