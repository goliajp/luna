//! The state of PUC's count luna keeps besides `totalbytes`.

use super::*;

/// PUC's structures luna has no object for, as far as the count goes.
pub(in crate::runtime) struct PucModel {
    /// PUC's `totalbytes` and when the next step is due
    pub(super) count: super::count::Count,
    /// the dialect PUC's sizes, pace and steps are those of
    pub(super) dialect: crate::runtime::dialect::Dialect,
    /// `collectgarbage("stop")`: no step runs on its own while true
    pub(super) stopped: bool,
    /// 5.4 and 5.5: a finalizer is running, which no step may interrupt
    /// (PUC `GCSTPGC`)
    pub(super) fin_stop: bool,
    /// the collection cycle under way, counted from 1 (wrapping: a thread
    /// would have to sit out 2^32 cycles to be taken as shrunk in this one)
    pub(super) cycle: u32,
    /// the rules the heap follows in `dialect`
    pub(super) rules: super::for_heap::HeapRules,
    /// `collectgarbage`'s pacing parameters
    pub(super) params: super::GcParams,
    /// the sizes of PUC's objects in this dialect, which `count.bytes`
    /// counts
    pub(super) sizes: super::size::PucSizes,
    /// PUC's collector state and pace
    pub(super) pace: super::pace::Pace,
    /// the records of objects PUC has and luna does not
    pub(super) records: super::records::Records,
    /// uncounted objects of luna's standing for objects of PUC's of the
    /// given size (see `Heap::stand_sizes_on`)
    pub(super) stand_ins: super::for_heap::StandIns,
    /// the loads under way, as the model sees them
    pub(super) loads: super::api::Loads,
    /// what loads made before PUC does, kept off the list
    pub(super) unborn: super::records::parse::Unborn,
    /// what the opening of the standard libraries made
    pub(super) opening: super::api::Opening,
}

impl Default for PucModel {
    fn default() -> PucModel {
        let d = crate::runtime::dialect::Dialect::L55;
        // nothing is counted until the dialect is known (`acct_boot`)
        PucModel {
            count: super::count::Count::new(),
            dialect: d,
            stopped: false,
            fin_stop: false,
            cycle: 1,
            params: super::GcParams::new(crate::version::LuaVersion::Lua55),
            sizes: super::size::PucSizes::of(d),
            rules: super::for_heap::HeapRules::of(d, &super::size::PucSizes::of(d)),
            pace: super::pace::Pace::new(),
            records: super::records::Records::new(),
            stand_ins: super::for_heap::StandIns::default(),
            loads: super::api::Loads::new(),
            unborn: super::records::parse::Unborn::new(),
            opening: super::api::Opening::default(),
        }
    }
}

impl Heap {
    /// Run `f` with counting off.
    #[inline]
    pub(in crate::runtime) fn uncounted<R>(&mut self, f: impl FnOnce(&mut Heap) -> R) -> R {
        let off = self.counting_off();
        let r = f(self);
        self.counting_on(off);
        r
    }
}
