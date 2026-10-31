//! The open upvalues PUC frees with their thread's stack: lent to the
//! marking, closed, freed and revived as PUC's are.

use super::super::*;
use crate::runtime::Coro;
use crate::runtime::function::{UpvalState, Upvalue};
use crate::runtime::puc::records::open_upvals::*;

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

    /// The lists handed over (see [`Heap::lend_open_upvals`]), back to the
    /// VM.
    #[inline]
    pub(crate) fn take_open_lend(&mut self) -> Option<OpenLend> {
        self.puc.records.open_dead.lend.take()
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
}
