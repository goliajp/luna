//! A side trace as the exit of its parent holds it: the side trace, and
//! the checks running it from that exit needs, worked out when it is
//! wired.

use super::super::{ENTRY_TAG_ANY, ExitTag};
use super::CompiledTrace;
use crate::jit::send_compat::TArc;

/// The side trace wired to an exit of its parent.
pub(crate) struct SideHeld {
    pub(crate) child: TArc<CompiledTrace>,
    /// The registers of the side trace, as the parent's window numbers
    /// them, that the parent's exit leaves untouched and the side trace
    /// reads with a tag it checks: when the parent took one unchecked, the
    /// stack holds its value, whose tag the dispatcher checks before
    /// running the side trace. With the tag each is compiled for.
    pub(crate) stack_reads: Box<[(u16, u8)]>,
}

impl SideHeld {
    /// `child`, wired to an exit of its parent whose tags are `at_exit`,
    /// its register 0 the parent's register `off`.
    pub(crate) fn new(child: TArc<CompiledTrace>, at_exit: &[ExitTag], off: usize) -> Self {
        let stack_reads = (0u16..)
            .zip(child.plans.entry_tags.iter())
            .filter(|&(i, &c)| {
                c != ENTRY_TAG_ANY
                    && at_exit
                        .get(off + usize::from(i))
                        .is_none_or(|&t| t == ExitTag::Untouched)
            })
            .map(|(i, &c)| ((off + usize::from(i)) as u16, c))
            .collect();
        SideHeld { child, stack_reads }
    }
}
