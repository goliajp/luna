//! Finding again a string the caller already has.

use super::*;

impl Heap {
    /// Whether a string of `len` bytes is interned: the table holds one
    /// string per content (short ones; in 5.1 every one).
    #[inline]
    pub(crate) fn interns(&self, len: usize) -> bool {
        len <= string::MAX_SHORT_LEN || self.rules.hash51()
    }

    /// [`Heap::try_intern`] of the bytes of `g`, the string the table
    /// holds for them: what finding it does, without the lookup.
    #[inline]
    pub(crate) fn refind(&mut self, g: Gc<LuaStr>) -> Gc<LuaStr> {
        #[cfg(feature = "gc-verify")]
        assert!(
            self.interns(g.len())
                && self
                    .strings
                    .find(g.as_bytes(), self.seed, self.rules.hash51())
                    == Some(g.as_ptr()),
            "a string found again is not the table's"
        );
        self.found_string(g, g.len())
    }
}
