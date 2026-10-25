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
        let upval = Upvalue {
            hdr: GcHeader::new(ObjTag::Upvalue),
            state,
        };
        let g = self.try_adopt(upval)?;
        self.puc_born(g, Made);
        Ok(g)
    }

    /// A new open upvalue of stack slot `slot` of `thread` (`None`: the
    /// main thread). 5.1 and 5.2 keep it off the list until it closes
    /// ([`Heap::upval_closed`]).
    #[inline]
    pub(crate) fn try_new_open_upvalue(
        &mut self,
        slot: u32,
        thread: Option<Gc<crate::runtime::Coro>>,
    ) -> Result<Gc<Upvalue>, Oom> {
        let p = self.alloc_block::<Upvalue>()?;
        // SAFETY: `p` is a fresh block for one `Upvalue`, which starts with
        // its header; kept off the list (white as a new object) or linked,
        // the heap owns it from here
        let g = unsafe {
            p.write(Upvalue {
                hdr: GcHeader::new(ObjTag::Upvalue),
                state: UpvalState::Open { slot, thread },
            });
            let h = p as *mut GcHeader;
            if self.puc_rules().open_upvals_on_threads {
                self.hook_reborn(h);
                (*h).next = list::ON_THREAD;
                (*h).paint(self.white());
            } else {
                self.link(h);
            }
            Gc::from_ptr(p)
        };
        self.puc_born(g, Made);
        Ok(g)
    }

    /// A closure of `proto` with its `n` upvalues still to find, each
    /// place holding a closed upvalue of nil (PUC `luaF_newLclosure`,
    /// whose places are empty): the VM puts each upvalue in as it finds
    /// it, the closure holding the ones found if a collection runs.
    pub(crate) fn try_new_closure_to_fill(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        n: usize,
    ) -> Result<Gc<crate::runtime::function::LuaClosure>, Oom> {
        let b = match self.blank_upval {
            Some(b) => b,
            None => self.make_blank_upval()?,
        };
        self.try_new_closure_of(proto, std::iter::repeat_n(b, n))
    }

    #[cold]
    fn make_blank_upval(&mut self) -> Result<Gc<Upvalue>, Oom> {
        let p = self.alloc_block::<Upvalue>()?;
        // SAFETY: `p` is a fresh block for one `Upvalue`, which starts with
        // its header; on the fixed list, colored neither white nor black,
        // it is never marked or swept, made without PUC's count, and goes
        // with the heap
        let g = unsafe {
            p.write(Upvalue {
                hdr: GcHeader::new(ObjTag::Upvalue),
                state: UpvalState::Closed(Value::Nil),
            });
            let h = p as *mut GcHeader;
            (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
            (*h).next = self.fixed;
            self.fixed = h;
            Gc::from_ptr(p)
        };
        self.blank_upval = Some(g);
        Ok(g)
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
        let size = self.puc_size(PucObj::Udata { nuv: 1, bytes: 0 });
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
        let g = self.try_adopt(u)?;
        self.puc_born(g, Made);
        Ok(g)
    }
}
