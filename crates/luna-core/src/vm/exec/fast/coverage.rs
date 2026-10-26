//! Which arm returned which [`Flow`] in an unoptimised build (where the
//! arms are functions, see `arm_fns`), so that a test can see that a run
//! went through every arm. An arm that falls through to its slow path
//! returns `Flow::Resume`; one that ran its instruction again instead would
//! show up in `arm_tests` as a metamethod called again.

use super::frame::Flow;
use super::*;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static SEEN: RefCell<HashSet<(u8, u8)>> = RefCell::new(HashSet::new());
}

/// The arm of `op` returned `flow`.
pub(super) fn note(op: Op, flow: &Flow) {
    let kind = match flow {
        Flow::Next => 0,
        Flow::NextJumped => 1,
        Flow::Resume => 2,
        Flow::ResumeSame => 3,
        Flow::Reenter => 4,
        Flow::Frames => 5,
        Flow::Returned(_) => 6,
        Flow::Exit(_) => 7,
    };
    SEEN.with(|s| s.borrow_mut().insert((op as u8, kind)));
}

macro_rules! table_ops {
    (() [$($op:ident $name:ident $body:expr;)*] [$($fo:ident)*] [$($sl:ident)*]) => {
        &[$(Op::$op),*]
    };
}

#[cfg(test)]
#[path = "coverage_tests.rs"]
mod tests;

/// Every opcode with an arm, from the table.
pub(super) const ARMS: &[Op] = fast_arms!(table_ops,);

/// What the runs since the last call saw, then forgotten.
pub(super) fn take() -> HashSet<(u8, u8)> {
    SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
}
