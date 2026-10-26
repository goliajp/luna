//! The block a `Gen`'s records live in.

use super::*;

/// A `Gen`'s six records, in a block of their own that is reached only
/// through the pointer kept here: the collector holds pointers to their
/// headers, which a reference to the block taken later would invalidate.
pub(crate) struct Phantoms(ptr::NonNull<Phantom>);

impl Phantoms {
    pub(super) fn new(ph: [Phantom; 6]) -> Phantoms {
        let p = Box::into_raw(Box::new(ph));
        Phantoms(ptr::NonNull::new(p.cast()).expect("a box is not null"))
    }

    /// Record `k`.
    pub(super) fn at(&self, k: usize) -> *mut Phantom {
        debug_assert!(k < 6);
        self.0.as_ptr().wrapping_add(k)
    }

    /// Record `k`'s header, at its start.
    pub(super) fn hdr(&self, k: usize) -> *mut GcHeader {
        self.at(k).cast()
    }

    /// `f` on record `k`.
    pub(super) fn with<R>(&self, k: usize, f: impl FnOnce(&mut Phantom) -> R) -> R {
        // SAFETY: the block lives while `self` does and every record in it
        // is initialised; the reference lasts for `f` alone, and only the
        // collector's pointers reach the block besides this one
        f(unsafe { &mut *self.at(k) })
    }
}

impl Phantom {
    /// The record swept with `w` the current white: `None` if it was dead
    /// before, else whether it is freed now (with whether it was counted)
    /// or kept.
    pub(super) fn sweep(&mut self, w: u8) -> Option<Option<bool>> {
        if self.dead {
            return None;
        }
        let f = self.hdr.flags();
        if is_white(f) && f & w == 0 {
            self.dead = true;
            self.hdr.set_flags(BLACK);
            Some(Some(self.counted))
        } else {
            self.hdr.set_flags(w);
            Some(None)
        }
    }
}

impl Drop for Phantoms {
    fn drop(&mut self) {
        // SAFETY: the block came from `Phantoms::new`'s box, given back once
        drop(unsafe { Box::from_raw(self.0.as_ptr().cast::<[Phantom; 6]>()) });
    }
}
