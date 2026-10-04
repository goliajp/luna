//! String interning, native functions, and the objects the heap fixes.

use super::*;
use crate::runtime::mem::{LSlice, Oom, oom_abort};
use std::alloc::Layout;

impl Heap {
    /// Create (or find) a string. Short strings (≤ 40 bytes) are interned.
    pub fn intern(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        self.try_intern(bytes).unwrap_or_else(|_| {
            oom_abort(Layout::array::<u8>(bytes.len()).unwrap_or(Layout::new::<u8>()))
        })
    }

    /// [`Heap::intern`], failing when memory runs out.
    #[inline]
    pub fn try_intern(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        if bytes.len() <= string::MAX_SHORT_LEN {
            let (p, is_new) = self.strings.intern(self.mem.mem(), bytes, self.seed)?;
            if is_new {
                // SAFETY: `StringTable::intern` just allocated `p` and put it only in its own bucket chain, which does not link objects
                unsafe { self.link(p as *mut GcHeader) };
                self.bytes += string::alloc_size(bytes.len());
            } else {
                // PUC `luaS_new` resurrect guard (lstring.c).
                // The bucket-chain is walked open-loop without consulting GC
                // color; during incremental sweep an existing entry may be
                // dead-white (in `sweep_cur`, scheduled for `free_obj`). If we
                // hand its pointer back, the budget-paced sweep frees it out
                // from under the mutator and the next bucket walk dereferences
                // a libc-recycled slot.
                //
                // Flip the white bits to `current_white` so the upcoming sweep
                // skips it (PUC `changewhite`). Black / not-white objects are
                // already safe and untouched.
                // SAFETY: `p` came from `StringTable::intern` and is a valid
                // `LuaStr` header (its bucket chain is consistent under our
                // single-threaded heap).
                unsafe {
                    let f = (*(p as *mut GcHeader)).flags;
                    if is_white(f) && (f & self.current_white) == 0 {
                        (*(p as *mut GcHeader)).flags = (*(p as *mut GcHeader))
                            .with_slow((f & !WHITE_BITS) | self.current_white);
                    }
                }
            }
            // SAFETY: `p` is an interned string the heap manages: new and linked above, or found in the table and kept from this cycle's sweep by the recoloring above
            Ok(unsafe { Gc::from_ptr(p) })
        } else {
            let p = string::alloc_long(self.mem.mem(), bytes, self.seed)?;
            // SAFETY: `alloc_long` just allocated `p`, linked nowhere yet
            unsafe { self.link(p as *mut GcHeader) };
            self.bytes += string::alloc_size(bytes.len());
            // SAFETY: `p` is the string linked just above
            Ok(unsafe { Gc::from_ptr(p) })
        }
    }

    /// Allocate a [`NativeClosure`] wrapping host function `f` with the
    /// given captured upvalues.
    pub fn new_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        self.new_native_from(f, &upvals)
    }

    /// [`Heap::new_native`] copying the upvalues from a slice.
    pub fn new_native_from(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
    ) -> Gc<NativeClosure> {
        self.try_new_native(f, upvals, false)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// Like [`Heap::new_native`] but tags the
    /// closure with `is_async = true`. The dispatcher's native-call
    /// path then transmutes `f` to `AsyncNativeFn` and routes through
    /// the cooperative-yield path. The caller is responsible for
    /// having transmuted the `AsyncNativeFn` pointer to `NativeFn`
    /// shape (both are `fn` pointers of the same size); see
    /// [`crate::vm::async_drive`] for the helper that does this.
    pub fn new_async_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Gc<NativeClosure> {
        self.try_new_native(f, &upvals, true)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// A native function over `f` with a copy of `upvals`, async when
    /// `is_async`, failing when memory runs out.
    pub fn try_new_native(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: &[Value],
        is_async: bool,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let fix = self.fix_natives && upvals.is_empty() && !is_async;
        let hdr = GcHeader::native(upvals);
        let kind = if is_async {
            crate::vm::exec::native_call::NativeKind::Async
        } else {
            crate::vm::exec::native_call::NativeKind::of(f)
        };
        let upvals = LSlice::from_slice(self.mem(), upvals)?;
        let g = self.try_adopt(NativeClosure {
            hdr,
            f,
            upvals,
            is_async,
            kind,
        })?;
        if fix {
            // SAFETY: `try_adopt` just linked `g` at the head of `all`
            unsafe { self.fix_newest(g.as_ptr() as *mut GcHeader) };
        }
        Ok(g)
    }

    /// PUC `luaC_fix`: move `h` from the head of `all` onto `fixed`, gray, so
    /// marking, barriers and weak tables skip it and only the heap's drop
    /// frees it.
    ///
    /// # Safety
    /// `h` is the object at the head of `all`.
    unsafe fn fix_newest(&mut self, h: *mut GcHeader) {
        debug_assert!(self.all == h);
        // SAFETY: the caller's contract: `h` is a live object, first on `all`
        unsafe {
            self.all = (*h).next;
            (*h).next = self.fixed;
            (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
        }
        self.fixed = h;
    }

    /// Make the fixed "not enough memory" string memory errors carry (PUC
    /// `luaS_init` makes `memerrmsg` first thing and fixes it).
    pub(super) fn make_memerr(&mut self) -> Result<(), Oom> {
        let s = self.try_intern(b"not enough memory")?;
        // SAFETY: a new heap's first string was linked at the head of `all`
        // by the intern just above
        unsafe { self.fix_newest(s.as_ptr() as *mut GcHeader) };
        self.mem_ctx().set_memerr(s);
        Ok(())
    }
}

#[cold]
pub(super) fn oom_abort_heap() -> ! {
    crate::runtime::mem::oom_abort(std::alloc::Layout::new::<Heap>())
}

/// A seed of a new heap's own, as [`Heap::new`] picks it.
pub(crate) fn fresh_seed() -> u32 {
    make_seed()
}

/// Hash seed from address entropy (ASLR) and clock, luaL_makeseed style.
pub(super) fn make_seed() -> u32 {
    let stack_var = 0u8;
    let mut h = &stack_var as *const u8 as u64;
    h ^= (make_seed as *const () as u64) << 16;
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h ^= (d.subsec_nanos() as u64) << 32 ^ d.as_secs();
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h as u32
}
