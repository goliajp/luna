//! What the heap's marking asks of the model: the work of marking a leaf,
//! which objects are luna's own, and the children of the model's records.

use super::*;
use crate::runtime::function::NativeClosure;
use crate::runtime::puc::records::env51::is_phantom;
use crate::runtime::puc::records::parse::is_load_tab;
use crate::runtime::userdata::UserdataPayload;

impl Marker {
    /// The children of the model's record behind `h`: 5.1's environment
    /// stand-in, or a table a load holds.
    ///
    /// # Safety
    /// `h` heads an allocated record of the model's (a `Ghost`), and
    /// nothing is freed while marking.
    #[inline]
    pub(in crate::runtime) unsafe fn puc_trace_record(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if is_phantom(h) {
                self.trace_phantom(h);
            } else if is_load_tab(h) {
                self.trace_load_tab(h);
            }
        }
    }

    /// Whether `h` is an object of luna's own that PUC does not have: a
    /// native standing for one of PUC's light C functions (from 5.2 on), a
    /// table of luna's own, a userdata standing for PUC's light userdata.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline]
    pub(in crate::runtime) unsafe fn puc_luna_only(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            match (*h).tag {
                ObjTag::Native => {
                    (*h).cls & LUNA_ONLY != 0
                        || !self.rules().native_is_object(&*(h as *const NativeClosure))
                }
                ObjTag::Table => (*h).cls & UNLISTED != 0,
                // the closure standing for PUC's parser's (`reader_layers.rs`)
                ObjTag::Closure => (*h).cls & LUNA_ONLY != 0,
                ObjTag::Userdata => {
                    (*h).is_uncounted()
                        && matches!((*(h as *const Userdata)).payload, UserdataPayload::Empty)
                }
                _ => false,
            }
        }
    }
}

impl HeapRules {
    /// Whether marking a leaf counts work (5.2 and 5.3, `GCmemtrav`).
    #[inline(always)]
    pub(in crate::runtime) fn counts_leaf_work(&self) -> bool {
        self.leaf_hdr != 0
    }

    /// The work of marking the leaf `h`: a string's size (PUC's
    /// `sizelstring`), a userdata's PUC size, 5.2's closed upvalue; 0 for
    /// anything of luna's own.
    #[inline(always)]
    pub(in crate::runtime) fn leaf_work(&self, h: Gc<GcHeader>) -> i64 {
        let h = h.as_ptr();
        // SAFETY: a handle's object is allocated; the tag names its type
        unsafe {
            match (*h).tag {
                // a string of luna's own is no string of PUC's
                ObjTag::Str if (*h).sub & UNCOUNTED != 0 => 0,
                ObjTag::Str => (self.leaf_hdr + (*(h as *const LuaStr)).len() + 1) as i64,
                // one standing for PUC's has PUC's size
                ObjTag::Userdata => (*(h as *const Userdata)).puc_size as i64,
                // 5.2: an open one stays gray, counting nothing
                ObjTag::Upvalue => match (*(h as *const crate::runtime::function::Upvalue)).state {
                    crate::runtime::function::UpvalState::Closed(_) => self.leaf_upval as i64,
                    _ => 0,
                },
                _ => 0,
            }
        }
    }
}
