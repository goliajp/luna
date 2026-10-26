//! Driving PUC's collector: its parameters and pace, its steps, and the
//! records its marking and sweep keep (5.1's environments, open upvalues).

use super::super::*;
use crate::runtime::Coro;
use crate::runtime::function::Upvalue;
use crate::runtime::heap::Heap;
use crate::runtime::puc::for_heap::FinalizerStop;
use crate::runtime::puc::records::open_upvals::OpenLend;
use crate::runtime::puc::step::CiCount;

/// Driving PUC's collector: its parameters and pace, its steps, and the
/// records its marking and sweep keep (5.1's environments, open upvalues).
pub(crate) trait PucCollector {
    /// What a memory cap is held against: the bytes the allocation context
    /// has handed out, when it counts them (a host function or a policy),
    /// else PUC's count ([`Heap::bytes`]).
    fn cap_bytes(&self) -> usize;
    /// Thread `owner` closes its upvalues from `slot` up: the dead ones
    /// among them are freed now.
    fn close_dead_open(&mut self, thread: Option<Gc<Coro>>, slot: u32);
    /// Whether an allocation that failed may run an emergency collection:
    /// never in 5.1; in 5.2 only while the collector runs (`gcrunning`);
    /// from 5.3 on, always.
    fn emergency_allowed(&self) -> bool;
    /// Where the latest records are on PUC's list: E1 and `io_fclose`
    /// after the io library's first function, E2 and `io_noclose` before
    /// its last, E3 and `io_pclose` before them (the standard files they
    /// were made after are at the list's end).
    fn env51_anchors(&mut self, first: *mut GcHeader, last: *mut GcHeader);
    /// Record `k` made now (the opening counted it): current white and
    /// live; its header, which names it on the stack. E1 made again
    /// starts the records of another opening.
    fn env51_born(&mut self, k: usize) -> *mut GcHeader;
    /// The default input (0) or output (1) in the latest E1's array.
    fn env51_io(&self, i: usize) -> Option<Gc<Userdata>>;
    /// The package table, the loaders' environment.
    fn env51_package(&mut self, t: Gc<Table>);
    /// Which record `h` heads, if one does.
    fn env51_record_index(&self, h: *mut GcHeader) -> Option<usize>;
    /// The latest E1's array entry `i` set to `u`, with the barrier of
    /// `lua_rawseti` into E1.
    fn env51_set_io(&mut self, i: usize, u: Gc<Userdata>);
    /// `lua_setfield` of `key` and the record `v` (a closure) into the
    /// record `t`, one of the io library's tables: 5.1's `luaC_barriert`.
    /// Both must be records of this heap's.
    fn env51_set_key(&mut self, t: *mut GcHeader, key: Gc<LuaStr>, v: *mut GcHeader);
    /// The finalizer returned, `ok` or raising: the collector runs as
    /// before it; a 5.1 error leaves the threshold it ran under.
    fn finalizer_returned(&mut self, stop: FinalizerStop, ok: bool);
    /// A finalizer is about to run (`GCTM`): the collector stops as PUC's
    /// does, 5.1 under a threshold twice the count, 5.2 and 5.3 not
    /// running, 5.4 and 5.5 stopped for the finalizer (`GCSTPGC`). What
    /// [`Heap::finalizer_returned`] puts back.
    fn finalizer_runs(&mut self) -> FinalizerStop;
    /// There may be objects to be finalized (PUC `gcfinnum = 1`).
    fn finalizers_due(&mut self);
    /// 5.2's full collection calls the pending finalizers first; the
    /// others clear the running stack's dead slots.
    fn full_calls_pending_first(&self) -> bool;
    /// [`Heap::gc_due`] once `more` bytes more are counted.
    fn gc_due_after(&self, more: usize) -> bool;
    /// `collectgarbage("stop"/"restart")`: suspend or resume auto-GC.
    fn gc_is_stopped(&self) -> bool;
    /// `collectgarbage`'s pacing parameters.
    fn gc_params(&self) -> &GcParams;
    fn gc_params_mut(&mut self) -> &mut GcParams;
    /// `collectgarbage("stop")` / `"restart"`. A restart makes a step due
    /// at once (5.1 `GCthreshold = totalbytes`, later `luaE_setdebt(g, 0)`).
    fn gc_set_stopped(&mut self, stopped: bool);
    /// Whether some dead open upvalue waits for the sweep.
    fn has_dead_open(&self) -> bool;
    /// The VM's lists of open upvalues, lent to the collection under way.
    fn lend_open_upvals(&mut self, lend: OpenLend);
    /// The light function the opening made as function `id`.
    fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>>;
    /// Whether the standalone interpreter restarts the collector in
    /// generational mode before it runs anything (5.4 on `lua.c`).
    fn lua_c_starts_generational(&self) -> bool;
    /// Whether this dialect's collector keeps open upvalues on threads.
    fn open_on_threads(&self) -> bool;
    /// Which `CallInfo`s PUC's traversal of a thread counts.
    fn puc_ci_count(&self) -> CiCount;
    /// The count at which the next step falls due, from the pace. 5.1
    /// stops the collector with an unreachable threshold; the later
    /// dialects still step then, to put the debt back.
    fn refresh_gc_limit(&mut self);
    /// The dead open upvalue of `owner`'s `slot`, which a new closure takes
    /// back before the sweep frees it (PUC `luaF_findupval`).
    fn revive_open(&mut self, thread: Option<Gc<Coro>>, slot: u32) -> Option<Gc<Upvalue>>;
    /// The object behind `h`, a native or a userdata, has environment
    /// `env`: one of the io library's is the latest opening's.
    fn set_env51(&mut self, h: Gc<GcHeader>, env: u8);
    /// Function `id` of the opening's `count` is the light function `g`:
    /// one value however often pushed.
    fn set_light(&mut self, id: usize, count: usize, g: Gc<crate::runtime::NativeClosure>);
    /// The lists handed over (see [`Heap::lend_open_upvals`]), back to the
    /// VM.
    fn take_open_lend(&mut self) -> Option<OpenLend>;
}

impl PucCollector for Heap {
    #[inline(always)]
    fn cap_bytes(&self) -> usize {
        Heap::cap_bytes(self)
    }

    #[inline(always)]
    fn close_dead_open(&mut self, thread: Option<Gc<Coro>>, slot: u32) {
        Heap::close_dead_open(self, thread, slot)
    }

    #[inline(always)]
    fn emergency_allowed(&self) -> bool {
        Heap::emergency_allowed(self)
    }

    #[inline(always)]
    fn env51_anchors(&mut self, first: *mut GcHeader, last: *mut GcHeader) {
        Heap::env51_anchors(self, first, last)
    }

    #[inline(always)]
    fn env51_born(&mut self, k: usize) -> *mut GcHeader {
        Heap::env51_born(self, k)
    }

    #[inline(always)]
    fn env51_io(&self, i: usize) -> Option<Gc<Userdata>> {
        Heap::env51_io(self, i)
    }

    #[inline(always)]
    fn env51_package(&mut self, t: Gc<Table>) {
        Heap::env51_package(self, t)
    }

    #[inline(always)]
    fn env51_record_index(&self, h: *mut GcHeader) -> Option<usize> {
        Heap::env51_record_index(self, h)
    }

    #[inline(always)]
    fn env51_set_io(&mut self, i: usize, u: Gc<Userdata>) {
        Heap::env51_set_io(self, i, u)
    }

    #[inline(always)]
    fn env51_set_key(&mut self, t: *mut GcHeader, key: Gc<LuaStr>, v: *mut GcHeader) {
        Heap::env51_set_key(self, t, key, v)
    }

    #[inline(always)]
    fn finalizer_returned(&mut self, stop: FinalizerStop, ok: bool) {
        Heap::finalizer_returned(self, stop, ok)
    }

    #[inline(always)]
    fn finalizer_runs(&mut self) -> FinalizerStop {
        Heap::finalizer_runs(self)
    }

    #[inline(always)]
    fn finalizers_due(&mut self) {
        Heap::finalizers_due(self)
    }

    #[inline(always)]
    fn full_calls_pending_first(&self) -> bool {
        Heap::full_calls_pending_first(self)
    }

    #[inline(always)]
    fn gc_due_after(&self, more: usize) -> bool {
        Heap::gc_due_after(self, more)
    }

    #[inline(always)]
    fn gc_is_stopped(&self) -> bool {
        Heap::gc_is_stopped(self)
    }

    #[inline(always)]
    fn gc_params(&self) -> &GcParams {
        Heap::gc_params(self)
    }

    #[inline(always)]
    fn gc_params_mut(&mut self) -> &mut GcParams {
        Heap::gc_params_mut(self)
    }

    #[inline(always)]
    fn gc_set_stopped(&mut self, stopped: bool) {
        Heap::gc_set_stopped(self, stopped)
    }

    #[inline(always)]
    fn has_dead_open(&self) -> bool {
        Heap::has_dead_open(self)
    }

    #[inline(always)]
    fn lend_open_upvals(&mut self, lend: OpenLend) {
        Heap::lend_open_upvals(self, lend)
    }

    #[inline(always)]
    fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>> {
        Heap::light(self, id)
    }

    #[inline(always)]
    fn lua_c_starts_generational(&self) -> bool {
        Heap::lua_c_starts_generational(self)
    }

    #[inline(always)]
    fn open_on_threads(&self) -> bool {
        Heap::open_on_threads(self)
    }

    #[inline(always)]
    fn puc_ci_count(&self) -> CiCount {
        Heap::puc_ci_count(self)
    }

    #[inline(always)]
    fn refresh_gc_limit(&mut self) {
        Heap::refresh_gc_limit(self)
    }

    #[inline(always)]
    fn revive_open(&mut self, thread: Option<Gc<Coro>>, slot: u32) -> Option<Gc<Upvalue>> {
        Heap::revive_open(self, thread, slot)
    }

    #[inline(always)]
    fn set_env51(&mut self, h: Gc<GcHeader>, env: u8) {
        Heap::set_env51(self, h, env)
    }

    #[inline(always)]
    fn set_light(&mut self, id: usize, count: usize, g: Gc<crate::runtime::NativeClosure>) {
        Heap::set_light(self, id, count, g)
    }

    #[inline(always)]
    fn take_open_lend(&mut self) -> Option<OpenLend> {
        Heap::take_open_lend(self)
    }
}
