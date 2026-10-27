//! What the heap asks of the model besides the counting entries
//! (`count.rs`, `parts.rs`) and the records' own: the count against the
//! next step, the pause a cycle ends with, the numbers objects hash by,
//! and what a cycle notes of threads and strings for PUC's structures.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::pace::{St, pause_debt, pause_debt_55, threshold_51};

impl Heap {
    /// PUC's `totalbytes`.
    #[inline(always)]
    pub(in crate::runtime) fn puc_bytes(&self) -> usize {
        self.puc.count.bytes
    }

    /// Whether the count reaches the next step's due point once `more`
    /// bytes more are counted.
    #[inline(always)]
    pub(in crate::runtime::puc) fn puc_due_after(&self, more: usize) -> bool {
        self.puc.count.bytes + more >= self.puc.count.gc_limit
    }

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

    /// Where the next step falls due, from the pace; never while 5.1's
    /// collector is `stopped_51` (an unreachable threshold).
    #[inline]
    pub(in crate::runtime::puc) fn puc_set_limit(&mut self, stopped_51: bool) {
        self.puc.count.gc_limit = if stopped_51 {
            usize::MAX
        } else {
            self.puc.pace.limit(self.puc.dialect)
        };
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
            self.puc.records.stand_ins.push((h, size));
        }
    }

    /// The objects of PUC's that `h`, an uncounted object of luna's, stood
    /// for are freed with it.
    pub(in crate::runtime::puc) fn free_stand_in(&mut self, h: *mut GcHeader) {
        while let Some(i) = self.puc.records.stand_ins.iter().position(|(p, _)| *p == h) {
            let (_, size) = self.puc.records.stand_ins.swap_remove(i);
            self.acct_part(size, 0);
        }
    }

    /// The bytes of PUC's string table (its array of buckets).
    #[inline]
    pub(in crate::runtime::puc) fn strt_bytes(&self) -> usize {
        self.puc.records.strt_size * self.puc.sizes.ptr
    }

    /// The light function the opening made as function `id`.
    pub(in crate::runtime) fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.puc.opening.lights.get(id).copied().flatten()
    }

    /// Function `id` of the opening's `count` is the light function `g`:
    /// one value however often pushed.
    pub(in crate::runtime) fn set_light(
        &mut self,
        id: usize,
        count: usize,
        g: Gc<crate::runtime::NativeClosure>,
    ) {
        let l = &mut self.puc.opening.lights;
        if l.len() <= id {
            l.resize(count.max(id + 1), None);
        }
        l[id] = Some(g);
    }
}

mod born;
mod cycle;
mod free;
#[cfg(any(feature = "gc-trace", feature = "gc-verify", test))]
pub(in crate::runtime) mod hooks;
mod marker;
mod pace;
pub(in crate::runtime) use pace::FinalizerStop;
mod rules;
mod sweep;
pub(in crate::runtime) use born::{
    C0, ExternalStr, FoundStr, Made, NativeFunction, NewStr, OfProto, PucFunction, RemadeStr,
    StandingFor,
};
pub(in crate::runtime) use rules::HeapRules;
pub(crate) use rules::ShrinkAt;
pub(crate) use sweep::Fate;

// the names of the model's records the heap goes by: it reaches the model
// through this module and the methods on `Heap`, never its other modules
pub(in crate::runtime) use crate::runtime::puc::records::Ghost;
pub(in crate::runtime) use crate::runtime::puc::records::parse::{
    KEPT, LoadTab, ParseFrame, kept_bit,
};
pub(in crate::runtime) use crate::runtime::puc::records::strt::SWEPT;
pub(in crate::runtime) use crate::runtime::puc::{PucKind, PucObj};
