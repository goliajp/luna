//! A short list kept in place: jump lists and assignment plans hold a few
//! entries, and allocating a vector for each costs more than the rest of
//! their handling.

use crate::runtime::mem::{LVec, MemRef};

/// A list of `T` whose first `N` items are stored in place.
pub(super) struct SmallList<T: Copy, const N: usize> {
    head: [Option<T>; N],
    len: usize,
    rest: LVec<T>,
}

impl<T: Copy, const N: usize> SmallList<T, N> {
    pub(super) fn new(mem: MemRef) -> Self {
        SmallList {
            head: [None; N],
            len: 0,
            rest: LVec::new(mem),
        }
    }

    pub(super) fn push(&mut self, v: T) {
        if self.len < N {
            self.head[self.len] = Some(v);
        } else {
            self.rest.push_or_abort(v);
        }
        self.len += 1;
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// The item at `i` (< `len`).
    pub(super) fn get(&self, i: usize) -> T {
        if i < N {
            self.head[i].expect("an item below len")
        } else {
            self.rest[i - N]
        }
    }

    /// The items in order.
    pub(super) fn iter(&self) -> impl Iterator<Item = T> + '_ {
        (0..self.len).map(|i| self.get(i))
    }
}

/// The pcs of jump instructions waiting for a target.
pub(super) type Jumps = SmallList<usize, 4>;

#[cfg(test)]
mod tests {
    use super::SmallList;
    use crate::runtime::mem::MemOwner;

    #[test]
    fn items_past_the_inline_ones_spill() {
        let o = MemOwner::system();
        let mut l: SmallList<u32, 2> = SmallList::new(o.mem());
        for i in 0..5 {
            l.push(i * 10);
        }
        assert_eq!(l.len(), 5);
        assert_eq!(l.iter().collect::<Vec<_>>(), vec![0, 10, 20, 30, 40]);
        assert_eq!(l.get(3), 30);
    }
}
