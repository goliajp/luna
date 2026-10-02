//! Upvalue cells.

use crate::runtime::heap::{Gc, GcHeader, Marker};
use crate::runtime::value::Value;

/// An upvalue cell. Open: refers to a live VM stack slot (the stack is a GC
/// root, so open cells trace nothing). Closed: owns the value inline.
#[repr(C)]
pub struct Upvalue {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    pub(crate) state: UpvalState,
}

/// Open / closed state of an upvalue cell.
#[derive(Clone, Copy)]
pub enum UpvalState {
    /// references slot `slot` of `thread`'s value stack (`None` = the main
    /// thread). The owning thread is tracked so the cell still resolves to the
    /// right stack after a coroutine swap.
    Open {
        /// Stack slot of the captured local on the owning thread.
        slot: u32,
        /// Owning thread, or `None` for the main thread.
        thread: Option<Gc<crate::runtime::coroutine::Coro>>,
    },
    /// Captured value has been hoisted into the cell.
    Closed(
        /// The closed-over value.
        Value,
    ),
}

impl Upvalue {
    /// Return the upvalue's current state (open / closed).
    pub fn state(&self) -> UpvalState {
        self.state
    }

    pub(crate) fn set_closed(&mut self, v: Value) {
        self.state = UpvalState::Closed(v);
    }

    // kept inline in the drain loop, which visits every upvalue
    #[inline(always)]
    pub(crate) fn trace(&self, m: &mut Marker) {
        match self.state {
            UpvalState::Closed(v) => {
                m.value(v);
            }
            UpvalState::Open {
                thread: Some(co), ..
            } => {
                m.header(co.as_ptr() as *mut GcHeader);
            }
            UpvalState::Open { thread: None, .. } => {}
        }
    }
}
