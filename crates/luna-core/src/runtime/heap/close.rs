//! Freeing what is left when the heap goes: nothing is counted, no list
//! or table is kept up, each block only goes back.

use super::*;

impl Heap {
    /// Free the object behind `h` as the heap goes; the kinds that need
    /// more than their blocks back go through [`Heap::free_obj`].
    ///
    /// # Safety
    /// As for `free_obj`, with the heap being dropped.
    pub(super) unsafe fn free_obj_closing(&mut self, h: *mut GcHeader) {
        debug_assert!(self.dropping);
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            match (*h).tag {
                ObjTag::Str => string::free(h as *mut LuaStr, self.mem()),
                // its slot goes back with the slab
                ObjTag::Native if (*h).cls & SLAB != 0 => {
                    ptr::drop_in_place(h as *mut NativeClosure)
                }
                ObjTag::Table if (*h).cls & STANDS == 0 => {
                    let t = h as *mut Table;
                    (*t).free_parts(self.mem());
                    self.free_block(t);
                }
                _ => self.free_obj(h),
            }
        }
    }
}
