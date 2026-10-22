//! Checks debug builds and the test-only builds run: the slow bits after
//! each atomic step and sweep, whether the collector holds an object.

use super::*;

impl Heap {
    #[allow(dead_code)]
    /// Whether `h` is an object of this heap the collector keeps: on the
    /// object lists, and not one a pending sweep frees. Walks every object;
    /// for checks tests turn on.
    pub(crate) fn holds_live(&self, h: *const GcHeader) -> bool {
        for head in [self.all, self.fixed] {
            let mut cur = head;
            while !cur.is_null() {
                if std::ptr::eq(cur, h) {
                    // SAFETY: an object on the lists is allocated
                    let f = unsafe { (*cur).flags };
                    let dying =
                        self.phase == GcPhase::Sweep && is_white(f) && f & self.current_white == 0;
                    return !dying;
                }
                // SAFETY: as above
                cur = unsafe { (*cur).next };
            }
        }
        false
    }

    /// Every object's SLOW bit agrees with its BLACK bit and read-only mark
    /// (`GcHeader::with_slow`), which every colour change keeps: a missed
    /// one would let an interpreter store skip the write barrier or write
    /// into a read-only table. Checked in debug builds after each atomic
    /// step and at the end of each sweep and full collection.
    #[allow(dead_code)]
    pub(in crate::runtime) fn verify_slow_bits(&self, ctx: &str) {
        let check = |h: *mut GcHeader| {
            // SAFETY: `h` is on one of the heap's own lists, all of whose
            // elements are live allocations; only the header is read
            let (ok, flags) = unsafe { (slow_consistent(&*h), (*h).flags) };
            assert!(ok, "[slow-bit] {ctx}: object {h:p} flags {flags:#x}");
        };
        for mut cur in [self.all, self.fixed] {
            while !cur.is_null() {
                check(cur);
                // SAFETY: as above
                cur = unsafe { (*cur).next };
            }
        }
        for &h in self.finalize.iter().chain(self.tobefnz_live()) {
            check(h);
        }
    }
}

/// Whether SLOW agrees with BLACK and the read-only mark.
fn slow_consistent(h: &GcHeader) -> bool {
    h.flags == h.with_slow(h.flags)
}
