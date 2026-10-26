//! What the model does at each phase of the heap's collection cycle.

use super::*;

impl Heap {
    /// The model takes on `version` and counts what PUC's `lua_newstate`
    /// allocates.
    pub(in crate::runtime) fn puc_start(
        &mut self,
        version: crate::version::LuaVersion,
    ) -> Result<(), crate::runtime::mem::Oom> {
        self.puc_dialect(version);
        self.acct_boot()
    }

    /// A cycle's marking starts: the model's records turn white with the
    /// objects.
    #[inline]
    pub(in crate::runtime) fn puc_mark_begins(&mut self) {
        self.env51_whiten();
    }

    /// What a marker borrows from the model: the sizer that counts what it
    /// marks (when the pace counts it), and the model's records (with the
    /// stand-ins, when `stands_unless_sized` and there is no sizer to hold
    /// them).
    #[inline]
    pub(in crate::runtime) fn puc_lend(
        &mut self,
        stands_unless_sized: bool,
    ) -> (Option<Sizer>, Lent) {
        let sizer = self.lend_sizer();
        let stands = stands_unless_sized && sizer.is_none();
        (sizer, self.lend_model(stands))
    }

    /// The atomic step ended with `fin` bytes of objects found to be
    /// finalized: the marker gives back what it borrowed, the trace lists
    /// the objects, and the string table's sweep is set up (an incremental
    /// cycle) or ended.
    pub(in crate::runtime) fn puc_atomic_ends(
        &mut self,
        fin: i64,
        m: &mut Marker,
        incremental: bool,
    ) {
        self.puc_found_to_finalize(fin);
        self.settle_sizer(m);
        self.hook_listed();
        if incremental {
            self.collect_dead_strings();
        } else {
            self.end_string_sweep();
        }
    }

    /// The atomic step's marking is over, every weak table cleared, just
    /// before the white flips: the string cache forgets the strings found
    /// dead (PUC `luaS_clearcache`), and the open upvalues left white come
    /// off their lists.
    pub(in crate::runtime) fn puc_marking_ends(&mut self) {
        self.puc.records.strcache.clear_dead();
        self.doom_open_upvals();
    }

    /// A full collection's sweep starts: 5.1's sweeps the string table
    /// before the list.
    pub(in crate::runtime) fn puc_full_sweep_begins(&mut self) {
        if self.puc.rules.full_sweeps_strings_first {
            self.collect_dead_strings();
            self.sweep_strings_upto(i64::MAX);
        }
    }

    /// The sweep of the list ended: the open upvalues it found dead go, and
    /// what loads set aside turns white as the survivors did.
    pub(in crate::runtime) fn puc_sweep_ends(&mut self) {
        self.sweep_open_end();
        self.whiten_unborn();
    }

    /// The heap is dropped: nothing is counted from here on, and the
    /// strings the string sweep held off the list come back to be freed,
    /// with the open upvalues off it.
    pub(in crate::runtime) fn puc_heap_drops(&mut self) -> Vec<*mut GcHeader> {
        self.counting_off().abandon();
        let mut held = self.take_held_strings();
        held.extend(self.open_off_list());
        // luna's names set aside, the only objects set aside now
        held.extend(self.unborn_objs());
        held
    }
}
