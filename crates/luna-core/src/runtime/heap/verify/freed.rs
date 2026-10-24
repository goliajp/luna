//! gc-verify: the addresses the collection freed, which the read probes
//! test, and the sweep's view of what PUC lists.

use super::*;

impl Heap {
    /// The first object never swept (`fixedgc`).
    #[inline]
    pub(in crate::runtime) fn fixed_head(&self) -> *mut GcHeader {
        self.fixed
    }

    /// The object `h` freed: noted for the read-time probes.
    pub(crate) fn note_freed(&mut self, h: Gc<GcHeader>) {
        let h = h.as_ptr();
        self.hooks.recently_freed.insert(h as usize);
        self.hooks.born_dead.remove(&(h as usize));
        crate::runtime::gc_verify_probe::FREED.with(|f| f.borrow_mut().insert(h as usize));
        // SAFETY: `h` comes from a handle, so the object is allocated; it is
        // read as a string when its tag says it is one
        let s = unsafe { ((*h).tag == ObjTag::Str).then(|| Gc::from_ptr(h as *mut LuaStr)) };
        if let Some(s) = s {
            let b = s.as_bytes().to_vec();
            crate::runtime::gc_verify_probe::FREED_STR
                .with(|f| f.borrow_mut().insert(h as usize, b));
        }
    }

    /// `gc-verify`: the object `h`, set aside with the flags `f`, goes on
    /// the list; one white but not the current white is garbage the
    /// marking now over did not reach.
    pub(in crate::runtime) fn note_born(&mut self, h: *mut GcHeader, f: u8) {
        if is_white(f) && f & self.current_white == 0 {
            self.hooks.born_dead.insert(h as usize);
        }
    }

    /// `gc-verify`: the address `h`, freed before, holds a new object.
    pub(in crate::runtime) fn forget_freed(&mut self, h: *mut GcHeader) {
        self.hooks.recently_freed.remove(&(h as usize));
        crate::runtime::gc_verify_probe::FREED.with(|f| f.borrow_mut().remove(&(h as usize)));
    }

    /// `gc-verify`: a new reference to the object at `h` (`what`) while a
    /// sweep runs must not be to one the sweep frees.
    pub(crate) fn verify_not_dying(&self, h: Gc<GcHeader>, what: &str) {
        let h = h.as_ptr().cast_const();
        // SAFETY: `h` comes from a handle, so the object is allocated; only
        // its flags are read
        let f = unsafe { (*h).flags };
        if self.phase == GcPhase::Sweep && is_white(f) && f & self.current_white == 0 {
            panic!(
                "[gc-verify] {what} {:#x} is dead, swept this cycle",
                h as usize
            );
        }
    }

    /// `gc-verify`: the color of the object at `h` against the heap's
    /// current white, and the collector's phase.
    pub(crate) fn describe(&self, h: Gc<GcHeader>) -> String {
        let h = h.as_ptr().cast_const();
        // SAFETY: `h` comes from a handle, so the object is allocated; only
        // its flags are read
        let f = unsafe { (*h).flags };
        format!(
            "flags {f:#x} white {:#x} phase {:?}",
            self.current_white, self.phase
        )
    }
}

/// [`Heap::verify_black_children`]'s check of child `h` of `parent`.
///
/// # Safety
/// `h` and `parent` head allocated objects.
pub(crate) unsafe fn dead_child(
    h: *mut GcHeader,
    (new_white, parent, ctx): (u8, *mut GcHeader, &'static str),
) {
    // SAFETY: the caller's contract; only headers are read
    unsafe {
        let f = (*h).flags;
        // a string only a load holds is kept by the sweep
        if is_white(f) && f & new_white == 0 && !crate::runtime::puc::for_heap::hooks::is_kept(h) {
            let what = if (*h).tag == ObjTag::Str {
                String::from_utf8_lossy(string::bytes_of(h as *const LuaStr)).into_owned()
            } else {
                String::new()
            };
            panic!(
                "[gc-verify] {ctx}: black {:?} {:#x} (cls {:#x}) holds {:?} {:#x} {what:?} (sub {:#x} cls {:#x}), which the sweep frees",
                (*parent).tag,
                parent as usize,
                (*parent).cls,
                (*h).tag,
                h as usize,
                (*h).sub,
                (*h).cls
            );
        }
    }
}
