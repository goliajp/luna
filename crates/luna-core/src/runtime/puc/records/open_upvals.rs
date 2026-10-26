//! 5.1 and 5.2 keep a thread's open upvalues on the thread, where only
//! closures mark them: an open upvalue no live closure holds is freed by
//! the sweep as it reaches its thread (the main thread after the others),
//! one a function closes before that is freed as it is closed, and one a
//! new closure finds is taken back. The live ones turn white there, and
//! one goes on the list of objects only as it closes. luna keeps open
//! upvalues off that list too, takes the dead ones off the open lists when
//! the marking ends, and frees each where PUC frees it.

use super::*;
use crate::runtime::Coro;
use crate::runtime::function::{UpvalState, Upvalue};
use crate::runtime::mem::LVec;

/// The header of `thread`, null for the main thread.
fn owner_of(thread: Option<Gc<Coro>>) -> *mut GcHeader {
    thread.map_or(ptr::null_mut(), |c| c.as_ptr().cast())
}

/// A thread's open upvalues, by stack slot.
pub(crate) type OpenList = LVec<(u32, Gc<Upvalue>)>;

/// The open lists the VM holds, handed over for the end of a marking and
/// handed back when the collection ends: the running thread's
/// (`running_co` null for the main thread) and, while a coroutine runs,
/// the main thread's. They move rather than being pointed at, as they
/// live in the VM, which the collector reaches only through the borrows
/// it is given.
pub(crate) struct OpenLend {
    pub(crate) running: OpenList,
    pub(crate) running_co: *mut GcHeader,
    pub(crate) main: Option<OpenList>,
}

/// The dead open upvalues the sweep has not reached yet.
#[derive(Default)]
pub(crate) struct OpenDead {
    pub(crate) lend: Option<OpenLend>,
    /// thread (null: the main one), slot, upvalue; each thread's from its
    /// highest slot down, the order of PUC's list
    dead: Vec<(*mut GcHeader, u32, Gc<Upvalue>)>,
}

impl Heap {
    /// The VM's lists of open upvalues, lent to the collection under way.
    #[inline]
    pub(crate) fn lend_open_upvals(&mut self, lend: OpenLend) {
        self.puc.records.open_dead.lend = Some(lend);
    }

    /// Whether this dialect's collector keeps open upvalues on threads.
    #[inline]
    pub(crate) fn open_on_threads(&self) -> bool {
        self.puc.rules.open_upvals_on_threads
    }

    /// The marking is over: the open upvalues it left white are dead, off
    /// their lists, and kept (black) until PUC frees them; the others take
    /// the white of the sweep to come.
    pub(in crate::runtime::puc) fn doom_open_upvals(&mut self) {
        let Some(mut lend) = self.puc.records.open_dead.lend.take() else {
            return;
        };
        if self.open_on_threads() {
            self.doom_lists(&mut lend);
        }
        self.puc.records.open_dead.lend = Some(lend);
    }

    /// The lists handed over (see [`Heap::lend_open_upvals`]), back to the
    /// VM.
    #[inline]
    pub(crate) fn take_open_lend(&mut self) -> Option<OpenLend> {
        self.puc.records.open_dead.lend.take()
    }

    fn doom_lists(&mut self, lend: &mut OpenLend) {
        // the white of the sweep about to start, which the live ones take
        // (PUC's sweep of the thread's list); none is on the list, so the
        // sweep does not reach it
        let live_white = self.white() ^ WHITE_BITS;
        let mut lists: Vec<(*mut GcHeader, *mut OpenList)> =
            vec![(lend.running_co, ptr::addr_of_mut!(lend.running))];
        if let Some(main) = &mut lend.main {
            lists.push((ptr::null_mut(), main));
        }
        for &co in self.threads() {
            let h = co.as_ptr() as *mut GcHeader;
            if h != lend.running_co {
                // SAFETY: a listed thread is allocated until the sweep
                lists.push((h, unsafe { ptr::addr_of_mut!((*co.as_ptr()).open_upvals) }));
            }
        }
        for (owner, list) in lists {
            // SAFETY: the lists handed over live in `lend`, borrowed for this
            // call, and a coroutine's is its own; nothing else holds them
            // meanwhile
            let list = unsafe { &mut *list };
            let start = self.puc.records.open_dead.dead.len();
            list.retain(|&(slot, uv)| {
                let h = uv.as_ptr() as *mut GcHeader;
                // SAFETY: a listed upvalue is allocated
                let f = unsafe { (*h).flags() };
                if !is_white(f) {
                    // SAFETY: as above
                    unsafe { (*h).paint(live_white) };
                    return true;
                }
                // SAFETY: as above; the block is kept until PUC frees it,
                // holding nothing (its thread may be freed first)
                unsafe {
                    (*h).paint(BLACK);
                    (*uv.as_ptr()).state = UpvalState::Closed(Value::Nil);
                }
                self.puc.records.open_dead.dead.push((owner, slot, uv));
                false
            });
            self.puc.records.open_dead.dead[start..].reverse();
        }
    }

    /// The sweep reaches `cur`: a thread's dead open upvalues are freed.
    ///
    /// # Safety
    /// `cur` heads an allocated object.
    #[inline]
    pub(in crate::runtime::puc) unsafe fn sweep_open_at(&mut self, cur: *mut GcHeader) {
        // SAFETY: the caller's contract
        if !self.puc.records.open_dead.dead.is_empty() && unsafe { (*cur).tag } == ObjTag::Coro {
            // 5.1's main thread is on the list too, before the userdata
            let main = self
                .main_thread_51()
                .is_some_and(|m| std::ptr::eq(m.as_ptr(), cur));
            let owner = if main { ptr::null_mut() } else { cur };
            self.free_dead_open(|o, _| o == owner);
        }
    }

    /// The sweep is over: the main thread's dead open upvalues are freed
    /// (and any whose thread it did not meet).
    pub(in crate::runtime::puc) fn sweep_open_end(&mut self) {
        if !self.puc.records.open_dead.dead.is_empty() {
            self.free_dead_open(|o, _| o.is_null());
            self.free_dead_open(|_, _| true);
        }
    }

    /// Whether some dead open upvalue waits for the sweep.
    #[inline]
    pub(crate) fn has_dead_open(&self) -> bool {
        !self.puc.records.open_dead.dead.is_empty()
    }

    /// Thread `owner` closes its upvalues from `slot` up: the dead ones
    /// among them are freed now.
    #[inline]
    pub(crate) fn close_dead_open(&mut self, thread: Option<Gc<Coro>>, slot: u32) {
        if !self.puc.records.open_dead.dead.is_empty() {
            let owner = owner_of(thread);
            self.free_dead_open(|o, s| o == owner && s >= slot);
        }
    }

    /// The dead open upvalue of `owner`'s `slot`, which a new closure takes
    /// back before the sweep frees it (PUC `luaF_findupval`).
    pub(crate) fn revive_open(
        &mut self,
        thread: Option<Gc<Coro>>,
        slot: u32,
    ) -> Option<Gc<Upvalue>> {
        if self.puc.records.open_dead.dead.is_empty() {
            return None;
        }
        let owner = owner_of(thread);
        let i = self
            .puc
            .records
            .open_dead
            .dead
            .iter()
            .position(|&(o, s, _)| o == owner && s == slot)?;
        let uv = self.puc.records.open_dead.dead.remove(i).2;
        // SAFETY: a dead open upvalue is kept until its free is counted;
        // it is off the list, and live again in the current white
        unsafe {
            (*uv.as_ptr()).state = UpvalState::Open { slot, thread };
            (*uv.header()).paint(self.white());
        }
        Some(uv)
    }

    #[cold]
    fn free_dead_open(&mut self, pick: impl Fn(*mut GcHeader, u32) -> bool) {
        let mut i = 0;
        while i < self.puc.records.open_dead.dead.len() {
            let (o, s, uv) = self.puc.records.open_dead.dead[i];
            if !pick(o, s) {
                i += 1;
                continue;
            }
            self.puc.records.open_dead.dead.remove(i);
            if !uv.hdr().is_uncounted() {
                self.acct_part(self.puc.sizes.upval, 0);
            }
            // SAFETY: the block is on no list, and only dead closures (which
            // free theirs without reading them) still point at it; counted
            // above, it is freed uncounted
            unsafe {
                (*uv.header()).set_uncounted(true);
                self.free_obj(uv.header());
            }
        }
    }

    /// The heap is dropped: the open upvalues off the list, those of the
    /// threads that are not running and the dead ones not freed yet (the
    /// VM puts its own lists' on the list as it is dropped).
    pub(in crate::runtime::puc) fn open_off_list(&mut self) -> Vec<*mut GcHeader> {
        let mut out: Vec<*mut GcHeader> = std::mem::take(&mut self.puc.records.open_dead.dead)
            .into_iter()
            .map(|(_, _, uv)| uv.header())
            .collect();
        if self.open_on_threads() {
            for &co in self.threads() {
                // SAFETY: the heap is dropped: nothing runs the thread or
                // borrows it; taken, its free closes none of them again
                let list = unsafe { co.as_mut() }.open_upvals.take();
                out.extend(list.iter().map(|&(_, uv)| uv.header()));
            }
        }
        out
    }
}
