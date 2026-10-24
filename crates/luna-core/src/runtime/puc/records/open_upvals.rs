//! 5.1 and 5.2 keep a thread's open upvalues on the thread, where only
//! closures mark them: an open upvalue no live closure holds is freed by
//! the sweep as it reaches its thread (the main thread after the others),
//! one a function closes before that is freed as it is closed, and one a
//! new closure finds is taken back. luna takes the dead ones off the open
//! lists when the marking ends, keeps their blocks for one more cycle, and
//! counts each free where PUC frees it.

use super::*;
use crate::runtime::Coro;
use crate::runtime::dialect::Dialect;
use crate::runtime::function::{UpvalState, Upvalue};
use crate::runtime::mem::LVec;

/// The header of `thread`, null for the main thread.
fn owner_of(thread: Option<Gc<Coro>>) -> *mut GcHeader {
    thread.map_or(ptr::null_mut(), |c| c.as_ptr().cast())
}

/// A thread's open upvalues, by stack slot.
pub(crate) type OpenList = LVec<(u32, Gc<Upvalue>)>;

/// The open lists the VM holds, lent for the end of a marking: the
/// running thread's (`running_co` null for the main thread) and, while a
/// coroutine runs, the main thread's.
#[derive(Clone, Copy)]
pub(crate) struct OpenLend {
    pub(crate) running: *mut OpenList,
    pub(crate) running_co: *mut GcHeader,
    pub(crate) main: *mut OpenList,
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
        self.puc.open_dead.lend = Some(lend);
    }

    /// Whether this dialect's collector keeps open upvalues on threads.
    #[inline]
    pub(crate) fn open_on_threads(&self) -> bool {
        matches!(self.puc.dialect, Dialect::L51 | Dialect::L52)
    }

    /// The marking is over: the open upvalues it left white are dead, off
    /// their lists, and kept (black) until their frees are counted.
    pub(in crate::runtime) fn doom_open_upvals(&mut self) {
        let Some(lend) = self.puc.open_dead.lend.take() else {
            return;
        };
        if !self.open_on_threads() {
            return;
        }
        let mut lists: Vec<(*mut GcHeader, *mut OpenList)> = vec![(lend.running_co, lend.running)];
        if !lend.main.is_null() {
            lists.push((ptr::null_mut(), lend.main));
        }
        for &co in self.threads() {
            let h = co.as_ptr() as *mut GcHeader;
            if h != lend.running_co {
                // SAFETY: a listed thread is allocated until the sweep
                lists.push((h, unsafe { ptr::addr_of_mut!((*co.as_ptr()).open_upvals) }));
            }
        }
        for (owner, list) in lists {
            // SAFETY: the lent lists are the VM's, alive for this call, and
            // a coroutine's is its own; nothing else holds them meanwhile
            let list = unsafe { &mut *list };
            let start = self.puc.open_dead.dead.len();
            list.retain(|&(slot, uv)| {
                let h = uv.as_ptr() as *mut GcHeader;
                // SAFETY: a listed upvalue is allocated
                let f = unsafe { (*h).flags };
                if !is_white(f) {
                    return true;
                }
                // SAFETY: as above; the block is kept through this sweep,
                // holding nothing (its thread may be freed by it)
                unsafe {
                    (*h).flags = (*h).with_slow((f & !COLOR_BITS) | BLACK);
                    (*uv.as_ptr()).state = UpvalState::Closed(Value::Nil);
                }
                self.puc.open_dead.dead.push((owner, slot, uv));
                false
            });
            self.puc.open_dead.dead[start..].reverse();
        }
    }

    /// The sweep reaches `cur`: a thread's dead open upvalues are freed.
    ///
    /// # Safety
    /// `cur` heads an allocated object.
    #[inline]
    pub(in crate::runtime) unsafe fn sweep_open_at(&mut self, cur: *mut GcHeader) {
        // SAFETY: the caller's contract
        if !self.puc.open_dead.dead.is_empty() && unsafe { (*cur).tag } == ObjTag::Coro {
            self.free_dead_open(|o, _| o == cur);
        }
    }

    /// The sweep is over: the main thread's dead open upvalues are freed
    /// (and any whose thread it did not meet).
    pub(in crate::runtime) fn sweep_open_end(&mut self) {
        if !self.puc.open_dead.dead.is_empty() {
            self.free_dead_open(|o, _| o.is_null());
            self.free_dead_open(|_, _| true);
        }
    }

    /// Whether some dead open upvalue waits for the sweep.
    #[inline]
    pub(crate) fn has_dead_open(&self) -> bool {
        !self.puc.open_dead.dead.is_empty()
    }

    /// Thread `owner` closes its upvalues from `slot` up: the dead ones
    /// among them are freed now.
    #[inline]
    pub(crate) fn close_dead_open(&mut self, thread: Option<Gc<Coro>>, slot: u32) {
        if !self.puc.open_dead.dead.is_empty() {
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
        if self.puc.open_dead.dead.is_empty() {
            return None;
        }
        let owner = owner_of(thread);
        let i = self
            .puc
            .open_dead
            .dead
            .iter()
            .position(|&(o, s, _)| o == owner && s == slot)?;
        let uv = self.puc.open_dead.dead.remove(i).2;
        // SAFETY: a dead open upvalue is kept until its free is counted
        unsafe { (*uv.as_ptr()).state = UpvalState::Open { slot, thread } };
        Some(uv)
    }

    #[cold]
    fn free_dead_open(&mut self, pick: impl Fn(*mut GcHeader, u32) -> bool) {
        let mut i = 0;
        while i < self.puc.open_dead.dead.len() {
            let (o, s, uv) = self.puc.open_dead.dead[i];
            if !pick(o, s) {
                i += 1;
                continue;
            }
            self.puc.open_dead.dead.remove(i);
            if !uv.hdr().is_uncounted() {
                self.acct_part(self.puc.sizes.upval, 0);
                // SAFETY: `uv` is a handle, so its upvalue is live; nothing
                // borrows the header while its per-type byte is written
                unsafe { (*uv.header()).set_uncounted(true) };
            }
        }
    }
}
