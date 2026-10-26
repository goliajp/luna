//! The open upvalue lists the VM hands the collector for the end of a
//! marking, and takes back when the collection ends: moved, not pointed
//! at, as they live in the VM.

use super::*;
use crate::runtime::puc::records::open_upvals::OpenLend;

impl Vm {
    /// The open lists the VM holds, handed to the collector for the end of
    /// this marking and back when the collection ends
    /// ([`Vm::take_back_open_lists`]).
    pub(crate) fn lend_open_lists(&mut self) {
        if !self.heap.open_on_threads() {
            return;
        }
        let running_co = self
            .current
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast());
        let main = match (&mut self.main_ctx, self.current) {
            (Some(mc), Some(_)) => Some(mc.open_upvals.take()),
            _ => None,
        };
        let running = self.open_upvals.take();
        self.heap.lend_open_upvals(OpenLend {
            running,
            running_co,
            main,
        });
    }

    /// The open lists [`Vm::lend_open_lists`] handed over, back.
    pub(crate) fn take_back_open_lists(&mut self) {
        let Some(lend) = self.heap.take_open_lend() else {
            return;
        };
        self.open_upvals = lend.running;
        if let (Some(mc), Some(main)) = (&mut self.main_ctx, lend.main) {
            mc.open_upvals = main;
        }
    }
}
