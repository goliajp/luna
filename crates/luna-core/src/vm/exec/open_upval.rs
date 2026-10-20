//! Finding the running thread's open upvalue of a stack slot.

use super::*;
use crate::runtime::function::Upvalue;

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
}
