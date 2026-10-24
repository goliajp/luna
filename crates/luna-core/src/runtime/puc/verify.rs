//! gc-verify's checks of the PUC model: the string table's count of
//! strings, the strings a load keeps for the sweep.

use super::*;
use crate::runtime::puc::records::ghost::{Ghost, SHORT, TAKEN};
use crate::runtime::puc::records::parse::KEPT;
use crate::runtime::puc::records::strt::SWEPT;

/// Whether `h` is a string only a load holds ([`KEPT`]), which the sweep
/// keeps.
///
/// # Safety
/// `h` heads an allocated object.
pub(in crate::runtime) unsafe fn is_kept(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Str && (*h).cls & KEPT != 0 }
}

impl Heap {
    /// `gc-verify`, around each step: what the model's state implies of
    /// luna's collector phase, and the reverse. Only implications that
    /// hold whatever path a cycle took: a full or emergency collection
    /// moves luna's phase without the model's, and luna's sweep may end
    /// before PUC's (the model then sweeps on with luna paused).
    pub(in crate::runtime) fn verify_phase_pair(&self, ctx: &str) {
        use crate::runtime::puc::pace::St;
        let (st, ph) = (self.puc.pace.state(), self.phase());
        let marking = matches!(st, St::Propagate | St::EnterAtomic | St::Atomic);
        let sweeping = matches!(
            st,
            St::SweepString
                | St::SweepUdata
                | St::SweepAll
                | St::SweepFinobj
                | St::SweepTobefnz
                | St::SweepEnd
        );
        let ok = match ph {
            // marking, in both
            GcPhase::Propagate => marking,
            // luna sweeps only while PUC does
            GcPhase::Sweep => sweeping,
            // a model in its pause, finalizing, or marking has luna paused,
            // propagating; luna paused with PUC still sweeping is the one
            // pairing a cycle's two sweeps allow
            GcPhase::Pause => !marking && !matches!(st, St::SweepString | St::SweepUdata),
        };
        assert!(
            ok,
            "[gc-verify] {ctx}: the model is in {st:?} with luna's collector in {ph:?}"
        );
    }

    /// `gc-verify`, once marking is done (the last sweep over): PUC's
    /// string table holds a place for each counted short string luna has
    /// and each one a record holds. Panics when the count went astray.
    pub(in crate::runtime) fn verify_string_count(&self, ctx: &str) {
        // and "not enough memory", which PUC makes at the start and luna
        // does not (a heap no Vm made has no PUC state: its string table
        // is never sized)
        let mut n = self.ghost_short_strings() + usize::from(self.puc.strt_size != 0);
        // and 5.1's strings luna's stand for
        let strt = crate::runtime::puc::STRT_STAND;
        n += self
            .puc
            .stand_ins
            .iter()
            .filter(|s| s.1 & strt != 0)
            .count();
        // SAFETY: a header on a list or set aside heads an allocated object
        let counted = |h: *mut GcHeader| unsafe {
            (*h).tag == ObjTag::Str
                && (*(h as *const LuaStr)).len() <= self.puc.short_max
                && (*h).sub & (UNCOUNTED | EXTERNAL | SWEPT) == 0
        };
        for mut cur in [self.list_head(), self.fixed_head()] {
            while !cur.is_null() {
                n += usize::from(counted(cur));
                // SAFETY: the lists link this heap's live objects
                cur = unsafe { (*cur).next };
            }
        }
        // and the strings a load made that are still set aside: interned
        // and counted, though off the lists
        n += self.unborn_objs().filter(|&h| counted(h)).count();
        assert_eq!(
            n, self.puc.strt_nuse,
            "{ctx}: counted short strings against PUC's string table"
        );
    }

    /// `gc-verify`: the short strings the live records hold, each a place
    /// in PUC's string table.
    pub(in crate::runtime) fn ghost_short_strings(&self) -> usize {
        let mut n = 0;
        let mut cur = self.list_head();
        // SAFETY: `all` links this heap's live objects; a header tagged
        // `Ghost` is a `Ghost`
        unsafe {
            while !cur.is_null() {
                if (*cur).tag == ObjTag::Ghost && (*cur).sub == 0 {
                    let sizes = &(*(cur as *const Ghost)).sizes;
                    n += sizes
                        .iter()
                        .filter(|&&s| s & (SHORT | TAKEN) == SHORT)
                        .count();
                }
                cur = (*cur).next;
            }
        }
        n
    }

    /// `gc-verify`: a string marked [`KEPT`] is one PUC frees in the sweep
    /// under way and luna keeps: a string of PUC's still counted, dead to
    /// this cycle (unmarked, not the current white), on the list the sweep
    /// has still to reach. The sweep turns it into an uncounted string of
    /// luna's and drops the mark; a marking that reaches it first drops the
    /// mark too. Panics on the first that is anything else.
    pub(in crate::runtime) fn verify_kept(&self, ctx: &str) {
        let mut cur = self.list_head();
        // SAFETY: the links of `all` lead to live objects; only headers are
        // read
        unsafe {
            while !cur.is_null() {
                let (tag, f, cls) = ((*cur).tag, (*cur).flags, (*cur).cls);
                if tag == ObjTag::Str && cls & KEPT != 0 {
                    let dead = is_white(f) && f & self.white() == 0;
                    assert!(
                        dead && !(*cur).is_uncounted() && self.phase() == GcPhase::Sweep,
                        "[gc-verify] {ctx}: kept string {cur:p}: flags {f:#x}, uncounted {}, phase {:?}",
                        (*cur).is_uncounted(),
                        self.phase()
                    );
                }
                cur = (*cur).next;
            }
        }
    }
}
