//! What the heap asks of the model besides the counting entries and the
//! records' own methods (see the model's module): the count against the
//! next step, the pause a cycle ends with, the work and sizes a cycle
//! counts, the model's bits in a header, and what a cycle notes of threads
//! and strings for PUC's structures.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::pace::{St, pause_debt, pause_debt_55, threshold_51};

/// Uncounted objects of luna's standing for objects of PUC's of the given
/// size, one entry for each object of PUC's.
pub(in crate::runtime::puc) struct StandIns(Vec<(*mut GcHeader, usize)>);

impl Default for StandIns {
    fn default() -> StandIns {
        // the standard libraries make about twenty
        StandIns(Vec::with_capacity(32))
    }
}

impl Heap {
    /// The collector restarts (`collectgarbage("restart")`): a step falls
    /// due at once (5.1 `GCthreshold = totalbytes`, later
    /// `luaE_setdebt(g, 0)`).
    pub(in crate::runtime::puc) fn puc_restart_due(&mut self) {
        let d = self.puc.dialect;
        match d {
            Dialect::L51 => self.puc.pace.set_threshold(self.puc.count.bytes as i64),
            _ => self.puc.pace.set_debt(d, self.puc.count.bytes, 0),
        }
    }

    /// A cycle ended with the count at what it is now: the pause before
    /// the next one, `pause` in percent of what survived (PUC `setpause`;
    /// 5.1 `setthreshold`), the collector in its pause.
    pub(in crate::runtime::puc) fn puc_pause(&mut self, pause: i64) {
        let d = self.puc.dialect;
        let bytes = self.puc.count.bytes;
        match d {
            Dialect::L51 => {
                self.puc.pace.estimate = bytes as i64;
                self.puc
                    .pace
                    .set_threshold(threshold_51(bytes as i64, pause));
            }
            Dialect::L55 => {
                let marked = bytes as i64 - self.fixed_bytes();
                self.puc.pace.estimate = marked;
                let debt = pause_debt_55(bytes, marked.saturating_mul(pause) / 100);
                self.puc.pace.set_debt(d, bytes, debt);
            }
            _ => {
                self.puc.pace.estimate = bytes as i64;
                let debt = pause_debt(d, bytes, bytes as i64, pause);
                self.puc.pace.set_debt(d, bytes, debt);
            }
        }
        self.puc.pace.enter(St::Pause);
    }

    /// The bytes of PUC's objects that no collection marks (`fixedgc`:
    /// the reserved words, the metamethod names, `_ENV`, the memory
    /// error's message).
    fn fixed_bytes(&self) -> i64 {
        let s = self.puc.sizes;
        let mut n: i64 = self
            .puc
            .records
            .pins
            .iter()
            .map(|p| s.string(p.len(), false) as i64)
            .sum();
        n += s.string(b"not enough memory".len(), false) as i64;
        n
    }

    /// 5.1: what the atomic step found to finalize (`udsize`), which the
    /// step's estimate leaves out.
    #[inline]
    pub(in crate::runtime::puc) fn puc_found_to_finalize(&mut self, bytes: i64) {
        self.puc.records.udsize = bytes;
    }

    /// 5.3: thread `h` comes up in this cycle's propagation; true the
    /// first time, when PUC's traversal shrinks its stack.
    pub(in crate::runtime) fn puc_shrinks_now(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.records.shrunk.contains(&h) {
            return false;
        }
        self.puc.records.shrunk.push(h);
        true
    }

    /// 5.2: the sweep kept thread `h`, which `sweepthread` shrinks.
    #[inline]
    pub(in crate::runtime::puc) fn puc_swept_thread(&mut self, h: *mut GcHeader) {
        self.puc.records.shrunk.push(h);
    }

    /// The uncounted object `h` stands for PUC's objects `objs`, freed with
    /// it.
    pub(in crate::runtime::puc) fn stand_for(&mut self, h: *mut GcHeader, objs: &[PucObj]) {
        for &o in objs {
            let size = self.puc_size(o);
            self.puc.stand_ins.0.push((h, size));
        }
    }

    /// The objects of PUC's that `h`, an uncounted object of luna's, stood
    /// for are freed with it.
    pub(in crate::runtime::puc) fn free_stand_in(&mut self, h: *mut GcHeader) {
        while let Some(i) = self.puc.stand_ins.0.iter().position(|(p, _)| *p == h) {
            let (_, size) = self.puc.stand_ins.0.swap_remove(i);
            self.acct_part(size, 0);
        }
    }

    /// The bytes of PUC's string table (its array of buckets).
    #[inline]
    pub(in crate::runtime::puc) fn strt_bytes(&self) -> usize {
        self.puc.records.strt_size * self.puc.sizes.ptr
    }
}

mod bits;
mod born;
mod cycle;
mod free;
#[cfg(any(feature = "gc-trace", feature = "gc-verify", test))]
pub(in crate::runtime) mod hooks;
mod marker;
mod sizer;
mod work;
pub(in crate::runtime) use sizer::Sizer;
mod rules;
mod sweep;
pub(in crate::runtime) use born::{
    C0, ExternalStr, FoundStr, Made, NativeFunction, NewStr, OfProto, PucFunction, RemadeStr,
    StandingFor,
};
pub(in crate::runtime) use rules::HeapRules;
pub(in crate::runtime) use rules::ShrinkAt;
pub(in crate::runtime) use sweep::Fate;

// the names of the model's records the heap goes by: it reaches the model
// through this module and the methods on `Heap`, never its other modules
pub(in crate::runtime) use crate::runtime::puc::records::Ghost;
pub(in crate::runtime) use crate::runtime::puc::records::parse::{LoadTab, ParseFrame};
pub(in crate::runtime) use crate::runtime::puc::{ENV51, Lent, PucModel};
pub(in crate::runtime) use crate::runtime::puc::{PucKind, PucObj};
pub(in crate::runtime) use bits::{STR_MODEL_BITS, object_marked, string_held, take_swept};
