//! Making upvalues and userdata.

use super::*;
use crate::runtime::mem::{Oom, oom_abort};
use std::alloc::Layout;

impl Heap {
    /// Allocate a fresh [`Upvalue`] cell in the given `state` (open / closed).
    pub fn new_upvalue(&mut self, state: UpvalState) -> Gc<Upvalue> {
        self.try_new_upvalue(state)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Upvalue>()))
    }

    /// [`Heap::new_upvalue`], failing when memory runs out.
    #[inline]
    pub fn try_new_upvalue(&mut self, state: UpvalState) -> Result<Gc<Upvalue>, Oom> {
        let size = self.puc_size(PucObj::Upval);
        let upval = Upvalue {
            hdr: GcHeader::new(ObjTag::Upvalue),
            state,
        };
        self.adopt_counted(upval, PucKind::Upval, size)
    }

    /// Create a userdata (an io file handle — luna's only userdata) with no
    /// metatable yet; the io library installs the shared `FILE*` metatable.
    pub fn new_userdata(&mut self, payload: UserdataPayload, writable: bool) -> Gc<Userdata> {
        self.try_new_userdata(payload, writable)
            .unwrap_or_else(|_| oom_abort(Layout::new::<Userdata>()))
    }

    /// [`Heap::new_userdata`], failing when memory runs out.
    pub fn try_new_userdata(
        &mut self,
        payload: UserdataPayload,
        writable: bool,
    ) -> Result<Gc<Userdata>, Oom> {
        // a userdata a host makes from Rust has no counterpart in PUC: one
        // with no block and a user value
        let size = self.puc_size(PucObj::udata(1, 0));
        self.try_new_userdata_sized(payload, writable, size)
    }

    /// [`Heap::try_new_userdata`] standing for PUC's userdata of `size`
    /// bytes (block and user values included).
    pub(crate) fn try_new_userdata_sized(
        &mut self,
        payload: UserdataPayload,
        writable: bool,
        size: usize,
    ) -> Result<Gc<Userdata>, Oom> {
        let mut hdr = GcHeader::new(ObjTag::Userdata);
        hdr.aux = self.take_id();
        let mut u = Userdata::new(hdr, payload, writable);
        u.puc_size = size;
        self.adopt_counted(u, PucKind::Userdata, size)
    }
}
