//! Calling natives: their arguments, the call itself and its invocation.

use super::*;

pub(in crate::vm::exec) mod args;
pub(crate) mod call;
pub(in crate::vm::exec) mod invoke;
