//! A side trace as the exit of its parent holds it: the side trace, and
//! what running it from that exit needs, worked out when it is wired or
//! the first time each of its exits is taken.

use super::super::{ENTRY_TAG_ANY, ExitTag};
use super::CompiledTrace;
use crate::jit::send_compat::TArc;

// shared between the Vms that run one trace's code, on any thread
type Once<T> = std::sync::OnceLock<T>;

/// The side trace wired to an exit of its parent.
pub(crate) struct SideHeld {
    pub(crate) child: TArc<CompiledTrace>,
    /// The registers of the side trace, as the parent's window numbers
    /// them, that the parent's exit leaves untouched and the side trace
    /// reads with a tag it checks: when the parent took one unchecked, the
    /// stack holds its value, whose tag the dispatcher checks before
    /// running the side trace. With the tag each is compiled for.
    pub(crate) stack_reads: Box<[(u16, u8)]>,
    /// Per exit of the side trace, numbered as its hit counts are: the
    /// write-back after it ran from this exit, worked out the first time.
    after: Box<[Once<ChildExit>]>,
}

/// The write-back after a side trace that starts at the parent's register
/// 0 ran and left at one of its exits: `written`, the registers that exit
/// wrote, with their kinds, and the registers past the parent's frame it
/// left untouched, as nil; and `entered`, the ones in the frame it left
/// untouched, written back with the tag they entered the parent with (the
/// parent may have written them before its exit). Covers `len` registers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ChildExit {
    pub(crate) written: Box<[(u16, ExitTag)]>,
    pub(crate) entered: Box<[u16]>,
    pub(crate) len: usize,
}

impl ChildExit {
    /// From a side trace's exit tags `tags`, in a parent whose frame has
    /// `max_stack` registers.
    pub(crate) fn of(tags: &[ExitTag], max_stack: usize) -> Self {
        // counted first, so each list is allocated once at its size
        let n = tags.iter().enumerate();
        let n_entered = n
            .filter(|&(i, &k)| k == ExitTag::Untouched && i < max_stack)
            .count();
        let mut written = Vec::with_capacity(tags.len() - n_entered);
        let mut entered = Vec::with_capacity(n_entered);
        for (slot, &kind) in (0u16..).zip(tags) {
            match kind {
                ExitTag::Untouched if usize::from(slot) >= max_stack => {
                    written.push((slot, ExitTag::Nil))
                }
                ExitTag::Untouched => entered.push(slot),
                _ => written.push((slot, kind)),
            }
        }
        ChildExit {
            written: written.into(),
            entered: entered.into(),
            len: tags.len(),
        }
    }
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
        let after = (0..child.plans.exit_count()).map(|_| Once::new()).collect();
        SideHeld {
            child,
            stack_reads,
            after,
        }
    }

    /// The write-back after the side trace left at its exit `i`, in a
    /// parent whose frame has `max_stack` registers.
    #[inline(always)]
    pub(crate) fn after(&self, i: usize, max_stack: usize) -> Option<&ChildExit> {
        let cell = self.after.get(i)?;
        Some(cell.get_or_init(|| ChildExit::of(self.child.plans.exit_tags(i), max_stack)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_exit_splits_written_and_entered_registers() {
        use ExitTag::*;
        let e = ChildExit::of(&[Int, Untouched, Float, Untouched, Untouched], 4);
        assert_eq!(&*e.written, &[(0, Int), (2, Float), (4, Nil)]);
        assert_eq!(&*e.entered, &[1, 3]);
        assert_eq!(e.len, 5);
    }
}
