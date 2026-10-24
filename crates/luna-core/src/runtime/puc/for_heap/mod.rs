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
        self.puc.bytes
    }

    /// Whether the count reaches the next step's due point once `more`
    /// bytes more are counted.
    #[inline(always)]
    pub(in crate::runtime) fn puc_due_after(&self, more: usize) -> bool {
        self.puc.bytes + more >= self.puc.gc_limit
    }

    /// The collector restarts (`collectgarbage("restart")`): a step falls
    /// due at once (5.1 `GCthreshold = totalbytes`, later
    /// `luaE_setdebt(g, 0)`).
    pub(in crate::runtime) fn puc_restart_due(&mut self) {
        let d = self.puc.dialect;
        match d {
            Dialect::L51 => self.puc.pace.set_threshold(self.puc.bytes as i64),
            _ => self.puc.pace.set_debt(d, self.puc.bytes, 0),
        }
    }

    /// Where the next step falls due, from the pace; never while 5.1's
    /// collector is `stopped_51` (an unreachable threshold).
    #[inline]
    pub(in crate::runtime) fn puc_set_limit(&mut self, stopped_51: bool) {
        self.puc.gc_limit = if stopped_51 {
            usize::MAX
        } else {
            self.puc.pace.limit(self.puc.dialect)
        };
    }

    /// A cycle ended with the count at what it is now: the pause before
    /// the next one, `pause` in percent of what survived (PUC `setpause`;
    /// 5.1 `setthreshold`), the collector in its pause.
    pub(in crate::runtime) fn puc_pause(&mut self, pause: i64) {
        let d = self.puc.dialect;
        let bytes = self.puc.bytes;
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
            .pins
            .iter()
            .map(|p| s.string(p.len(), false) as i64)
            .sum();
        n += s.string(b"not enough memory".len(), false) as i64;
        n
    }

    /// The number a new table, function, thread or userdata gets: its hash
    /// as a table key. PUC hashes these by address, which varies from run
    /// to run; numbers in creation order give one of the layouts PUC can
    /// have, the same every run.
    #[inline(always)]
    pub(in crate::runtime) fn take_id(&mut self) -> u32 {
        self.puc.next_id = self.puc.next_id.wrapping_add(16);
        self.puc.next_id
    }

    /// 5.1: what the atomic step found to finalize (`udsize`), which the
    /// step's estimate leaves out.
    #[inline]
    pub(in crate::runtime) fn puc_found_to_finalize(&mut self, bytes: i64) {
        self.puc.udsize = bytes;
    }

    /// 5.3: thread `h` comes up in this cycle's propagation; true the
    /// first time, when PUC's traversal shrinks its stack.
    pub(in crate::runtime) fn puc_shrinks_now(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.shrunk.contains(&h) {
            return false;
        }
        self.puc.shrunk.push(h);
        true
    }

    /// 5.2: the sweep kept thread `h`, which `sweepthread` shrinks.
    #[inline]
    pub(in crate::runtime) fn puc_swept_thread(&mut self, h: *mut GcHeader) {
        self.puc.shrunk.push(h);
    }

    /// PUC `luaS_clearcache`: the cache forgets the strings this cycle
    /// found dead.
    #[inline]
    pub(in crate::runtime) fn puc_clear_string_cache(&mut self) {
        self.puc.strcache.clear_dead();
    }

    /// The uncounted object `h` stands for objects of PUC's of the sizes in
    /// `sizes`, freed with it.
    pub(in crate::runtime) fn stand_for(
        &mut self,
        h: *mut GcHeader,
        sizes: impl Iterator<Item = usize>,
    ) {
        self.puc.stand_ins.extend(sizes.map(|s| (h, s)));
    }

    /// The objects of PUC's that `h`, an uncounted object of luna's, stood
    /// for are freed with it.
    pub(in crate::runtime) fn free_stand_in(&mut self, h: *mut GcHeader) {
        while let Some(i) = self.puc.stand_ins.iter().position(|(p, _)| *p == h) {
            let (_, size) = self.puc.stand_ins.swap_remove(i);
            if size & STRT_STAND != 0 {
                self.puc.strt_nuse -= 1;
            }
            self.acct_part(size & !STRT_STAND, 0);
        }
    }

    /// The bytes of PUC's string table (its array of buckets).
    #[inline]
    pub(in crate::runtime) fn strt_bytes(&self) -> usize {
        self.puc.strt_size * self.puc.sizes.ptr
    }

    /// The light function the opening made as function `id`.
    pub(crate) fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.puc.lights.get(id).copied().flatten()
    }

    /// Function `id` of the opening is the light function `g`: one value
    /// however often pushed.
    pub(crate) fn set_light(&mut self, id: usize, g: Gc<crate::runtime::NativeClosure>) {
        let l = &mut self.puc.lights;
        if l.len() <= id {
            l.resize(crate::vm::opening::FN_COUNT.max(id + 1), None);
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
mod rules;
mod sweep;
pub(in crate::runtime) use born::{
    C0, ExternalStr, FoundStr, Made, NativeFunction, NewStr, OfProto, PucFunction, RemadeStr,
    StandingFor,
};
pub(in crate::runtime) use rules::HeapRules;
pub(crate) use sweep::Fate;

// the names of the model's records the heap goes by: it reaches the model
// through this module and the methods on `Heap`, never its other modules
pub(in crate::runtime) use crate::runtime::puc::records::Ghost;
pub(in crate::runtime) use crate::runtime::puc::records::env51::is_phantom;
#[cfg(feature = "gc-trace")]
pub(in crate::runtime) use crate::runtime::puc::records::env51::phantom_is_table;
pub(in crate::runtime) use crate::runtime::puc::records::parse::{
    KEPT, LoadTab, ParseFrame, is_load_tab, kept_bit,
};
pub(in crate::runtime) use crate::runtime::puc::records::strt::SWEPT;
pub(in crate::runtime) use crate::runtime::puc::{PucKind, PucObj};
