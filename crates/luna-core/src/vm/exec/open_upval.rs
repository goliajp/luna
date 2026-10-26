//! Finding the running thread's open upvalue of a stack slot.

use super::*;
use crate::runtime::function::Upvalue;
use crate::runtime::mem::LVec;
use crate::runtime::puc::api::PucCollector;

impl Vm {
    /// The open upvalue of `slot`: listed, or (5.1 / 5.2) one a marking
    /// found dead that a new closure takes back, listed again; `Err` with
    /// where a new one goes. Interpreted and compiled code both find
    /// upvalues here.
    pub(crate) fn open_upval(&mut self, slot: u32) -> Result<Gc<Upvalue>, usize> {
        let i = match self.open_upvals.binary_search_by_key(&slot, |&(s, _)| s) {
            Ok(i) => return Ok(self.open_upvals[i].1),
            Err(i) => i,
        };
        if !self.heap.has_dead_open() || self.open_upvals.reserve(1).is_err() {
            return Err(i);
        }
        match self.heap.revive_open(self.current, slot) {
            Some(uv) => {
                self.open_upvals.insert(i, (slot, uv)).expect("reserved");
                Ok(uv)
            }
            None => Err(i),
        }
    }

    /// The open upvalue of stack slot `slot`, made and listed when there
    /// is none; a memory error when it cannot be.
    pub(crate) fn try_find_or_create_upval(&mut self, slot: u32) -> Result<Gc<Upvalue>, LuaError> {
        let i = match self.open_upval(slot) {
            Ok(uv) => return Ok(uv),
            Err(i) => i,
        };
        let thread = self.current;
        let listed = self.open_upvals.len();
        let uv = self.retry_oom(|vm| vm.heap.try_new_open_upvalue(slot, thread))?;
        // the new upvalue is young while it is not listed yet; an emergency
        // collection meanwhile may take dead ones off the list (5.1, 5.2)
        let at = |l: &LVec<(u32, Gc<Upvalue>)>| {
            if l.len() == listed {
                i
            } else {
                l.partition_point(|&(s, _)| s < slot)
            }
        };
        if let Err(e) = self.retry_oom(|vm| vm.open_upvals.insert(at(&vm.open_upvals), (slot, uv)))
        {
            self.heap.upval_unlisted(uv);
            return Err(e);
        }
        Ok(uv)
    }
}
