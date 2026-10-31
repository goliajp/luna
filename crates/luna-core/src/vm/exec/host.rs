//! What the host's API does on the VM: its arguments, operations, protected
//! calls, random numbers and stack access.

use super::*;

pub(in crate::vm::exec) mod api;
pub(in crate::vm::exec) mod arg;
pub(in crate::vm::exec) mod ops;
pub(crate) mod pcall;
pub(in crate::vm::exec) mod rng;
