//! What each exit of a trace writes back, worked out from its exit tags the
//! first time the exit is taken: the registers it wrote, with their kinds.

use super::super::ExitTag;

// shared between the Vms that run one trace's code, on any thread
type Once<T> = std::sync::OnceLock<T>;

/// A register an exit wrote: its slot in the trace's window and its kind.
/// Laid out so that generated code can read the plan as well.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExitSlot {
    pub(crate) slot: u16,
    pub(crate) kind: ExitTag,
}

/// One exit's plan: the registers it wrote, in slot order; every register
/// it left untouched lies below `untouched_end`; its tags cover `len`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedExit {
    pub(crate) written: Box<[ExitSlot]>,
    pub(crate) untouched_end: usize,
    pub(crate) len: usize,
}

impl PlannedExit {
    fn of(tags: &[ExitTag]) -> Self {
        // counted first, so the list is allocated once at its size
        let n = tags.iter().filter(|&&k| k != ExitTag::Untouched).count();
        let mut written = Vec::with_capacity(n);
        let mut untouched_end = 0;
        for (slot, &kind) in (0u16..).zip(tags) {
            if kind == ExitTag::Untouched {
                untouched_end = usize::from(slot) + 1;
            } else {
                written.push(ExitSlot { slot, kind });
            }
        }
        PlannedExit {
            written: written.into_boxed_slice(),
            untouched_end,
            len: tags.len(),
        }
    }
}

/// Per exit, numbered as its hit counts are (the inline side exits, the
/// typed exits of the tag table, then the trace's global tags), its plan
/// once the exit has been taken: an exit never taken costs one empty word.
#[derive(Debug, Default)]
pub(crate) struct ExitPlan {
    exits: Box<[Once<Box<PlannedExit>>]>,
}

impl ExitPlan {
    /// A plan for `n` exits, none worked out yet.
    pub(crate) fn new(n: usize) -> Self {
        ExitPlan {
            exits: (0..n).map(|_| Once::new()).collect(),
        }
    }

    /// The exits numbered.
    pub(crate) fn len(&self) -> usize {
        self.exits.len()
    }

    /// Bytes the per-exit cells take, without the plans worked out in them.
    pub(crate) fn cells_size(&self) -> usize {
        self.exits.len() * std::mem::size_of::<Once<Box<PlannedExit>>>()
    }

    /// Exit `i`'s plan, from `tags`, the exit's tags, the first time.
    #[inline]
    pub(crate) fn exit(&self, i: usize, tags: &[ExitTag]) -> Option<&PlannedExit> {
        Some(
            self.exits
                .get(i)?
                .get_or_init(|| Box::new(PlannedExit::of(tags))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_lists_the_registers_it_wrote_once_taken() {
        use ExitTag::*;
        let p = ExitPlan::new(2);
        let e = p.exit(0, &[Int, Untouched, Float]).expect("exit 0");
        assert_eq!(
            &*e.written,
            &[
                ExitSlot { slot: 0, kind: Int },
                ExitSlot {
                    slot: 2,
                    kind: Float
                }
            ]
        );
        assert_eq!((e.untouched_end, e.len), (2, 3));
        // worked out once: later tags are not read again
        assert_eq!(p.exit(0, &[]).expect("exit 0").len, 3);
        let g = p.exit(1, &[Untouched, Nil]).expect("exit 1");
        assert_eq!(
            (&*g.written, g.untouched_end),
            (&[ExitSlot { slot: 1, kind: Nil }][..], 1)
        );
        assert!(p.exit(2, &[Int]).is_none());
    }
}
