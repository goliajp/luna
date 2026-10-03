//! Writes to a table's array part that keep its count of non-nil slots
//! and the length of its leading run of them in step.

use super::*;

impl Table {
    /// [`Self::aset`] with the value read in place, as its tag byte and its
    /// payload word (see [`Value::copy_raw`]).
    ///
    /// # Safety
    /// `idx < asize`, and `pv` points at an initialised `Value`.
    #[inline(always)]
    pub(crate) unsafe fn aset_at(&mut self, idx: usize, pv: *const Value) {
        let p = pv as *const u8;
        // SAFETY: the caller's contract; a `Bool`'s byte and, from `Int`
        // on, the payload word are initialised (see `Value::unpack`)
        let (t, b) = unsafe {
            let t = *p;
            if t <= crate::runtime::value::tag::BOOL {
                (t + (t != 0 && *p.add(8) != 0) as u8, RawVal::NIL)
            } else {
                (t + 1, *(p.add(8) as *const RawVal))
            }
        };
        // SAFETY: `idx < asize` by the caller's contract, and both slices
        // are `asize` long
        let old = unsafe {
            let old = *self.atags().get_unchecked(idx);
            *self.atags_mut().get_unchecked_mut(idx) = t;
            *self.avals_mut().get_unchecked_mut(idx) = b;
            old
        };
        self.note_atag_change(idx, old, t);
    }

    /// Keep `acount` / `aprefix` in step with one array-slot tag change.
    #[inline]
    pub(super) fn note_atag_change(&mut self, idx: usize, old: u8, new: u8) {
        if old == raw::NIL && new != raw::NIL {
            self.acount += 1;
            if idx == self.aprefix as usize {
                // run over slots filled earlier, bounded so refilling a
                // hole low in a long array stays O(1); a shorter prefix
                // only disables the shortcut until the next `resize`
                let asize = self.asize();
                let atags = self.atags();
                let stop = (idx + 64).min(asize);
                let mut p = idx + 1;
                while p < stop && atags[p] != raw::NIL {
                    p += 1;
                }
                self.aprefix = p as u32;
            }
        } else if old != raw::NIL && new == raw::NIL {
            self.acount -= 1;
            if idx < self.aprefix as usize {
                self.aprefix = idx as u32;
            }
        }
    }

    /// Recompute `acount` / `aprefix` from the tag bytes.
    pub(super) fn recount_array(&mut self) {
        let atags = self.atags();
        let count = atags.iter().filter(|&&t| t != raw::NIL).count();
        let prefix = atags
            .iter()
            .position(|&t| t == raw::NIL)
            .unwrap_or(atags.len());
        self.acount = count as u32;
        self.aprefix = prefix as u32;
    }
}
