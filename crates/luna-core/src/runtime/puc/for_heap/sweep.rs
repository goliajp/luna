//! What the heap's sweep asks of the model at each object: the records and
//! open upvalues PUC sweeps with it, whether PUC's sweep counts it, and
//! what becomes of a dead one.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::userdata::UserdataPayload;

/// What the sweep does with a dead object.
pub(crate) enum Fate {
    /// freed, with the PUC objects swept after it (counted)
    Free(i64),
    /// kept: PUC keeps it, or one of its records needs it
    Keep,
    /// held for the string sweep under way: the heap only unlinks it
    Hold,
    /// set aside: a name of luna's PUC frees, made again where PUC makes
    /// it (see `Heap::name_aside_again`); the heap takes it off the list
    Aside,
}

impl Heap {
    /// The sweep reaches `cur`: the open upvalues of a thread PUC sweeps
    /// with it, and 5.1's records swept before it. Returns how many PUC
    /// objects that sweeps.
    ///
    /// # Safety
    /// `cur` heads an object of this heap that is still allocated.
    #[inline(always)]
    pub(in crate::runtime) unsafe fn puc_sweep_reach(&mut self, cur: *mut GcHeader) -> i64 {
        // SAFETY: the caller's contract
        unsafe {
            self.sweep_open_at(cur);
            self.env51_sweep(cur, false)
        }
    }

    /// The sweep kept `cur`: 5.2's `sweepthread`, and 5.1's records swept
    /// after it. Returns how many PUC objects that sweeps.
    ///
    /// # Safety
    /// As [`Heap::puc_sweep_reach`].
    #[inline(always)]
    pub(in crate::runtime) unsafe fn puc_sweep_kept(&mut self, cur: *mut GcHeader) -> i64 {
        // SAFETY: the caller's contract
        unsafe {
            if self.puc.dialect == Dialect::L52 && (*cur).tag == ObjTag::Coro {
                self.puc_swept_thread(cur);
            }
            self.env51_sweep(cur, true)
        }
    }

    /// `cur` is dead: freed, kept, or (`to_live`, a sweep to the first kept
    /// object) held for the string sweep under way, whose table holds it.
    /// What goes before its free is done here; the heap unlinks and frees.
    ///
    /// # Safety
    /// As [`Heap::puc_sweep_reach`].
    #[inline(always)]
    pub(in crate::runtime) unsafe fn puc_sweep_dead(
        &mut self,
        cur: *mut GcHeader,
        to_live: bool,
    ) -> Fate {
        // SAFETY: the caller's contract
        unsafe {
            if to_live && self.in_string_sweep(cur) {
                self.hold_for_string_sweep(cur);
                return Fate::Hold;
            }
            if self.kept_dead(cur) {
                return if self.name_aside_again(cur) {
                    Fate::Aside
                } else {
                    Fate::Keep
                };
            }
            let n = self.env51_sweep(cur, true);
            self.env51_forget(cur);
            Fate::Free(n)
        }
    }

    /// Whether PUC's sweep counts the object behind `h`, on `all`: one of
    /// PUC's (in 5.1 its main thread too, and the library functions and
    /// standard files luna made without counting). The objects with
    /// finalizers are on lists of their own, but 5.1's userdata.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    #[inline]
    pub(in crate::runtime) unsafe fn puc_counts_in_sweep(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            let (sub, cls) = ((*h).sub, (*h).cls);
            if self.puc.dialect == Dialect::L51 {
                let own = match (*h).tag {
                    ObjTag::Coro => false,
                    ObjTag::Native => cls & LUNA_ONLY != 0,
                    // the standard files, made without counting
                    ObjTag::Userdata => {
                        sub & UNCOUNTED != 0
                            && !matches!(
                                (*(h as *const Userdata)).payload,
                                UserdataPayload::File(_)
                            )
                    }
                    _ => sub & UNCOUNTED != 0,
                };
                cls & UNLISTED == 0 && !own
            } else {
                // luna's own natives are no objects of PUC's
                (sub & UNCOUNTED) | (cls & (UNLISTED | LUNA_ONLY)) == 0
            }
        }
    }

    /// A record of PUC's objects the counted sweep reaches, `dead` or not,
    /// with `left` of its quota: how many objects it counts, and, when it
    /// was dead (and not kept), whether its block may go (`None`: kept,
    /// the sweep goes on with it as with any object).
    ///
    /// # Safety
    /// `cur` heads a record of this heap that is still allocated.
    pub(in crate::runtime) unsafe fn puc_sweep_record(
        &mut self,
        cur: *mut GcHeader,
        dead: bool,
        left: i64,
    ) -> (i64, Option<bool>) {
        // SAFETY: the caller's contract
        unsafe {
            if dead && !self.kept_dead(cur) {
                let (k, gone) = self.free_ghost_records(&mut *(cur as *mut Ghost), left);
                return (k, Some(gone));
            }
            (Self::ghost_objects(&*(cur as *mut Ghost)), None)
        }
    }
}
