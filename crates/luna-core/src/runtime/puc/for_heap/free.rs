//! The model's part of freeing an object: its bytes off PUC's count, the
//! stand-in it had, and the records that are the model's own. One impl of
//! [`Freed`] per type, so the heap's free path, which already knows the
//! type, reaches its code without a second dispatch.

use super::*;
use crate::runtime::Coro;
use crate::runtime::function::{LuaClosure, NativeClosure, Proto};
use crate::runtime::puc::records::parse::is_load_tab;
use crate::runtime::upvalue::Upvalue;

/// What PUC's count loses when the heap frees an object of this type.
pub(in crate::runtime) trait Freed {
    fn header(&self) -> &GcHeader;
    fn freed(&self, heap: &mut Heap);
}

impl Heap {
    /// `obj` is about to be freed: PUC's count loses what it counted for
    /// it, and a stand-in for it goes. The heap frees its block after.
    #[inline(always)]
    pub(in crate::runtime) fn puc_freed<T: Freed>(&mut self, obj: &T) {
        let h = obj.header();
        if h.cls & STANDS != 0 {
            self.free_stand_in(h as *const GcHeader as *mut GcHeader);
        }
        obj.freed(self);
    }

    /// The record of the model's behind `h` is freed, whole.
    ///
    /// # Safety
    /// `h` heads an unlinked record of this heap (a `Ghost`) that nothing
    /// uses afterwards.
    pub(in crate::runtime) unsafe fn puc_free_record(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).cls & STANDS != 0 {
                self.free_stand_in(h);
            }
            if is_load_tab(h) {
                self.free_load_tab(h);
            } else {
                self.free_ghost(h as *mut Ghost);
            }
        }
    }
}

impl Freed for Table {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if self.hdr.cls & UNLISTED == 0 {
            heap.acct_table_free(self.asize as usize, self.node_count());
        }
    }
}

impl Freed for Proto {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if !self.hdr.is_uncounted() {
            heap.acct_proto_free(self);
        }
    }
}

impl Freed for LuaClosure {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if !self.hdr.is_uncounted() {
            // the prototype may be freed already
            let env = self.hdr.sub & ENV51 != 0;
            let n = self.upvals_len as usize - usize::from(env);
            heap.acct_part(heap.puc_size(PucObj::LuaClosure(n)), 0);
        }
    }
}

impl Freed for Upvalue {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if !self.hdr.is_uncounted() {
            heap.acct_part(heap.puc_size(PucObj::Upval), 0);
        }
    }
}

impl Freed for NativeClosure {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if self.hdr.is_uncounted() {
            heap.free_stand_in(&self.hdr as *const GcHeader as *mut GcHeader);
            return;
        }
        if heap.puc.rules.native_is_object(self) {
            // the upvalues PUC's function has, when luna keeps others
            let n = crate::runtime::puc::native_puc_upvals(self);
            heap.acct_part(heap.puc_size(PucObj::CClosure(n)), 0);
        }
    }
}

impl Freed for Coro {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if !self.hdr.is_uncounted() {
            let ci = heap.puc.rules.thread_ci(self);
            heap.acct_thread_free(self.stack_size, ci);
        }
    }
}

impl Freed for Userdata {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        if self.hdr.is_uncounted() {
            heap.free_stand_in(&self.hdr as *const GcHeader as *mut GcHeader);
        } else {
            heap.acct_part(self.puc_size, 0);
        }
    }
}

impl Freed for LuaStr {
    #[inline(always)]
    fn header(&self) -> &GcHeader {
        &self.hdr
    }
    #[inline(always)]
    fn freed(&self, heap: &mut Heap) {
        let sub = self.hdr.sub;
        if sub & EXTERNAL != 0 {
            // its block goes back first, then its header
            heap.trace_raw(self.len() + 1, 0);
            heap.acct_part(heap.puc_size(PucObj::ExternalStr), 0);
        } else if sub & FIXED_EXT != 0 {
            heap.acct_part(heap.puc_size(PucObj::LongStrHeader), 0);
        } else if sub & SWEPT != 0 {
            // the string sweep took it off the count
        } else if sub & UNCOUNTED != 0 {
            heap.free_stand_in(&self.hdr as *const GcHeader as *mut GcHeader);
        } else if !heap.is_dropping() {
            heap.acct_string_free(self.len());
        }
    }
}
