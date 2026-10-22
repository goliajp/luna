//! The accounting trace's view of 5.1's environment records (`gc-trace`
//! only).

use super::*;

impl Heap {
    /// How many of 5.1's environment records a sweep has not freed.
    pub(in crate::runtime) fn env51_live(&self) -> i64 {
        self.puc.env51_box.as_ref().map_or(0, |e| e.live())
    }
}

impl Env51 {
    /// How many of the records a sweep has not freed.
    pub(in crate::runtime) fn live(&self) -> i64 {
        let live = |g: &Gen| g.ph.iter().filter(|p| !p.dead).count() as i64;
        self.gens.iter().map(|g| live(g)).sum()
    }
}

/// Whether the record `h` stands for a table (else a closure).
///
/// # Safety
/// `h` heads a record.
pub(in crate::runtime) unsafe fn phantom_is_table(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*(h as *const Phantom)).kids[0] != NONE }
}
