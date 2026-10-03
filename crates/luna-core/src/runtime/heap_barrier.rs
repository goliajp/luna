//! Write barriers.

use super::*;

impl Heap {
    /// Forward write barrier: when a BLACK `parent` acquires a fresh reference
    /// to a WHITE `child`, gray the child (strings go straight to BLACK as
    /// leaves) and push onto the persistent gray queue so the next propagate
    /// step traces it. Mirrors PUC `luaC_barrier_`. No-op outside Propagate
    /// (parent is gray or white — the mutator never sees a BLACK object live
    /// outside an incremental cycle).
    #[inline(always)]
    pub fn barrier_forward<T: GcObject>(&mut self, parent: Gc<T>, child: Value) {
        // SAFETY: `parent` is a handle, so its object is allocated, and a `GcObject` starts with its header
        unsafe { self.barrier_forward_header(parent.header(), child) }
    }

    /// [`Self::barrier_forward`] on the parent's header. One body for every
    /// object type, so callers inline only the call.
    ///
    /// # Safety
    /// `parent` is the header of an allocated object.
    unsafe fn barrier_forward_header(&mut self, parent: *mut GcHeader, child: Value) {
        // SAFETY: `parent` heads an allocated object (the caller's contract) and `child` holds a live handle; only flag bytes are read and written, no reference to either object is formed
        unsafe {
            if !is_black((*parent).flags) {
                return;
            }
            let ch = match child {
                Value::Str(s) => s.as_ptr() as *mut GcHeader,
                Value::Table(t) => t.as_ptr() as *mut GcHeader,
                Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                Value::Native(n) => n.as_ptr() as *mut GcHeader,
                Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                _ => return,
            };
            let cf = (*ch).flags;
            if !is_white(cf) {
                return;
            }
            if (*ch).tag == ObjTag::Str {
                (*ch).flags = (cf & !COLOR_BITS) | BLACK;
            } else {
                (*ch).flags = cf & !WHITE_BITS;
                self.gray.push(ch);
            }
        }
    }

    /// Backward write barrier for objects with many fields (tables, threads):
    /// demote the parent itself back to gray so propagate re-traces it.
    /// Mirrors PUC `luaC_barrierback_`. One call covers any number of
    /// subsequent stores until the next propagate finishes — much cheaper for
    /// tables than per-child forward barriers. No-op outside Propagate.
    #[inline(always)]
    pub fn barrier_back<T: GcObject>(&mut self, parent: Gc<T>) {
        // SAFETY: `parent` is a handle, so its object is allocated, and a `GcObject` starts with its header
        unsafe { self.barrier_back_header(parent.header()) }
    }

    /// [`Self::barrier_back`] on the parent's header, one body for every
    /// object type.
    ///
    /// # Safety
    /// `parent` is the header of an allocated object.
    unsafe fn barrier_back_header(&mut self, parent: *mut GcHeader) {
        // SAFETY: `parent` heads an allocated object (the caller's contract); only its flag byte is read and written
        unsafe {
            let f = (*parent).flags;
            if !is_black(f) {
                return;
            }
            (*parent).flags = f & !COLOR_BITS;
            self.gray.push(parent);
        }
    }
}
