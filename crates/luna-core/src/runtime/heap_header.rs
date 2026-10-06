//! The flag and kind accessors of an object's header.

use super::*;

impl GcHeader {
    /// Whether this is a read-only table (`Table::is_readonly`).
    #[inline(always)]
    pub(crate) fn readonly(&self) -> bool {
        self.tag == ObjTag::Table && self.aux & READONLY_AUX != 0
    }

    /// Whether an in-place store may write into this object as it is:
    /// neither black (a black table needs the write barrier) nor a
    /// read-only table. One bit (SLOW) answers both, so the interpreter's
    /// store fast paths pay one bit test for both; the stores it turns away
    /// go to the slow path, which tells the two apart.
    #[inline(always)]
    pub(crate) fn plain_store(&self) -> bool {
        self.flags & SLOW == 0
    }

    /// Mark or unmark a table read-only, keeping SLOW in step.
    #[inline]
    pub(crate) fn set_readonly(&mut self, on: bool) {
        debug_assert!(self.tag == ObjTag::Table);
        if on {
            self.aux |= READONLY_AUX;
        } else {
            self.aux &= !READONLY_AUX;
        }
        self.flags = self.with_slow(self.flags);
    }

    /// Flag byte `f`, about to replace this header's, with SLOW set to
    /// agree with its BLACK bit and the read-only mark. Every write of the
    /// colour bits goes through here.
    #[inline(always)]
    pub(crate) fn with_slow(&self, f: u8) -> u8 {
        if f & BLACK != 0 || self.readonly() {
            f | SLOW
        } else {
            f & !SLOW
        }
    }

    /// Whether SLOW agrees with BLACK and the read-only mark.
    #[cfg(any(debug_assertions, feature = "gc-verify"))]
    pub(crate) fn slow_consistent(&self) -> bool {
        self.flags == self.with_slow(self.flags)
    }

    pub(crate) fn new(tag: ObjTag) -> GcHeader {
        GcHeader {
            next: ptr::null_mut(),
            tag,
            flags: if tag == ObjTag::Str { LEAF } else { 0 },
            sub: 0,
            aux: 0,
        }
    }

    /// A native function's header; one without upvalues has nothing to trace.
    #[inline]
    pub(super) fn native(upvals: &[Value]) -> GcHeader {
        GcHeader {
            flags: if upvals.is_empty() { LEAF } else { 0 },
            ..GcHeader::new(ObjTag::Native)
        }
    }
}
