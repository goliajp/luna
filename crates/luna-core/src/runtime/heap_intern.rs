//! String interning, native functions, and the objects the heap fixes.

use super::*;
use crate::runtime::Builtin;
use crate::runtime::mem::{LSlice, Oom, oom_abort};
use crate::runtime::value::NativeFn;
use std::alloc::Layout;

impl Heap {
    /// Create (or find) a string. Short strings (≤ 40 bytes) are interned,
    /// in 5.1 every string.
    pub fn intern(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        self.try_intern(bytes).unwrap_or_else(|_| {
            oom_abort(Layout::array::<u8>(bytes.len()).unwrap_or(Layout::new::<u8>()))
        })
    }

    /// Recolour the dead-white `h` to the current white, out of line: the
    /// hot path of [`Self::try_intern`] only tests the colour, and inlined,
    /// this cold store made it keep `self` on the stack.
    #[cold]
    #[inline(never)]
    fn resurrect(&self, h: &mut GcHeader) {
        h.flags = h.with_slow((h.flags & !WHITE_BITS) | self.current_white);
    }

    /// [`Heap::intern`], failing when memory runs out.
    #[inline]
    pub fn try_intern(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        match self.intern_or_none(bytes) {
            Some(s) => Ok(s),
            None => Err(Oom(self.mem.mem())),
        }
    }

    /// [`Heap::try_intern`] with the failure as `None`: one register to
    /// return, where the `Result` takes two.
    pub(crate) fn intern_or_none(&mut self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        // 5.1 interns strings of every length
        if bytes.len() <= string::MAX_SHORT_LEN || self.hash51 {
            let (p, is_new) = self
                .strings
                .intern(self.mem.mem(), bytes, self.seed, self.hash51)
                .ok()?;
            if is_new {
                // SAFETY: `StringTable::intern` just allocated `p` and put it only in its own bucket chain, which does not link objects
                unsafe { self.link(p as *mut GcHeader) };
                #[cfg(feature = "gc-trace")]
                if self.trace.where_ {
                    let b = String::from_utf8_lossy(bytes).into_owned();
                    self.trace.line(format_args!("s+ {b:?}"));
                }
                self.acct_new_string(p as *mut GcHeader, bytes.len());
            } else {
                return Some(self.found_string(p, bytes.len()));
            }
            // SAFETY: `p` is an interned string the heap manages, new and linked above
            Some(unsafe { Gc::from_ptr(p) })
        } else {
            let p = string::alloc_long(self.mem.mem(), bytes, self.seed, self.hash51).ok()?;
            // SAFETY: `alloc_long` just allocated `p`, linked nowhere yet
            unsafe { self.link(p as *mut GcHeader) };
            self.acct_new_string(p as *mut GcHeader, bytes.len());
            // SAFETY: `p` is the string linked just above
            Some(unsafe { Gc::from_ptr(p) })
        }
    }

    /// The one-byte string of `bytes` when it exists, found without a
    /// lookup (`string.sub` makes many).
    #[inline]
    pub(crate) fn find_single(&mut self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        let [c] = *bytes else { return None };
        let p = self.strings.single(c);
        (!p.is_null()).then(|| self.found_string(p, 1))
    }

    /// An interned string `p` of `len` bytes found again.
    #[inline]
    fn found_string(&mut self, p: *mut LuaStr, len: usize) -> Gc<LuaStr> {
        if is_uncounted(p as *mut GcHeader) {
            self.acct_promote(p as *mut GcHeader, len);
        }
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
                self.resurrect(&mut *(p as *mut GcHeader));
            }
            // found in the table and kept from this cycle's sweep by the
            // recoloring above
            Gc::from_ptr(p)
        }
    }

    /// The string of `a` then `b` (a concatenation), without a buffer to
    /// put them together in first.
    pub(crate) fn try_intern_pair(&mut self, a: &[u8], b: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        let n = a.len() + b.len();
        if n <= string::MAX_SHORT_LEN {
            let mut buf = [0u8; string::MAX_SHORT_LEN];
            buf[..a.len()].copy_from_slice(a);
            buf[a.len()..n].copy_from_slice(b);
            return self.try_intern(&buf[..n]);
        }
        if self.hash51 {
            // 5.1 looks every string up by its bytes
            let mut buf = [0u8; 256];
            if n <= buf.len() {
                buf[..a.len()].copy_from_slice(a);
                buf[a.len()..n].copy_from_slice(b);
                return self.try_intern(&buf[..n]);
            }
            return self.try_intern(&[a, b].concat());
        }
        let p = string::alloc_long_pair(self.mem.mem(), a, b, self.seed, self.hash51)?;
        // SAFETY: `alloc_long_pair` just allocated `p`, linked nowhere yet
        unsafe { self.link(p as *mut GcHeader) };
        self.acct_new_string(p as *mut GcHeader, n);
        // SAFETY: `p` is the string linked just above
        Ok(unsafe { Gc::from_ptr(p) })
    }

    /// The string of `parts` one after another (PUC `luaV_concat`'s
    /// result): looked up when short (5.1: always), else a new long string
    /// the parts are copied into.
    pub(crate) fn try_intern_parts(&mut self, parts: &[&[u8]]) -> Result<Gc<LuaStr>, Oom> {
        let n: usize = parts.iter().map(|p| p.len()).sum();
        if n <= string::MAX_SHORT_LEN {
            let mut buf = [0u8; string::MAX_SHORT_LEN];
            let mut at = 0;
            for p in parts {
                buf[at..at + p.len()].copy_from_slice(p);
                at += p.len();
            }
            return self.try_intern(&buf[..n]);
        }
        if self.hash51 {
            return self.try_intern(&parts.concat());
        }
        let p = string::alloc_long_parts(self.mem.mem(), parts, self.seed)?;
        // SAFETY: `alloc_long_parts` just allocated `p`, linked nowhere yet
        unsafe { self.link(p as *mut GcHeader) };
        self.acct_new_string(p as *mut GcHeader, n);
        // SAFETY: `p` is the string linked just above
        Ok(unsafe { Gc::from_ptr(p) })
    }

    /// 5.5 `lua_pushexternalstring` of `bytes` (longer than any short
    /// string), the block holding them handed over by a `luaL_Buffer`: a
    /// long string whose header alone `totalbytes` counts.
    pub(crate) fn new_external_string(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        let p = string::alloc_long_parts(self.mem.mem(), &[bytes], self.seed)?;
        // SAFETY: `alloc_long_parts` just allocated `p`, linked nowhere yet
        unsafe { self.link(p as *mut GcHeader) };
        if self.puc.silent > 0 {
            acct::set_uncounted(p as *mut GcHeader, true);
        } else {
            // SAFETY: as above
            unsafe { (*p).hdr.sub |= EXTERNAL };
            self.acct_obj(PucKind::LongStr, self.sizes.lngstr_hdr + 2 * self.sizes.ptr);
        }
        // SAFETY: `p` is the string linked just above
        Ok(unsafe { Gc::from_ptr(p) })
    }

    /// The interned string with `bytes`, if one has been made; never
    /// allocates. A short name no string exists for is no table's key.
    pub fn find_interned(&self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        if bytes.len() > string::MAX_SHORT_LEN && !self.hash51 {
            return None;
        }
        let p = self.strings.find(bytes, self.seed, self.hash51)?;
        // SAFETY: `p` is a string in the intern table, so allocated; a
        // dead-white one found mid-sweep is kept from this cycle's sweep by
        // the recoloring `try_intern` does
        unsafe {
            let f = (*(p as *mut GcHeader)).flags;
            if is_white(f) && (f & self.current_white) == 0 {
                return None;
            }
            Some(Gc::from_ptr(p))
        }
    }

    /// A [`NativeClosure`] calling host function `f` over `upvals`.
    pub fn new_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Gc<NativeClosure> {
        self.new_builtin(f, &upvals, Builtin::None)
    }

    /// [`Heap::new_native`] copying the upvalues from a slice.
    pub fn new_native_from(&mut self, f: NativeFn, upvals: &[Value]) -> Gc<NativeClosure> {
        self.new_builtin(f, upvals, Builtin::None)
    }

    /// A native that is the library function `builtin`.
    pub(crate) fn new_builtin(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        builtin: Builtin,
    ) -> Gc<NativeClosure> {
        self.try_new_builtin(f, upvals, builtin, false)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// [`Heap::new_native`] with `is_async` set: `f` is an `AsyncNativeFn`
    /// the caller transmuted to `NativeFn` ([`crate::vm::async_drive`]).
    pub fn new_async_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Gc<NativeClosure> {
        self.try_new_builtin(f, &upvals, Builtin::None, true)
            .unwrap_or_else(|_| oom_abort(Layout::new::<NativeClosure>()))
    }

    /// A native function over `f` with a copy of `upvals`, async when
    /// `is_async`, failing when memory runs out.
    pub fn try_new_native(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        is_async: bool,
    ) -> Result<Gc<NativeClosure>, Oom> {
        self.try_new_builtin(f, upvals, Builtin::None, is_async)
    }

    /// [`Heap::try_new_native`] for the library function `builtin`.
    pub(crate) fn try_new_builtin(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        builtin: Builtin,
        is_async: bool,
    ) -> Result<Gc<NativeClosure>, Oom> {
        let fix = self.fix_natives && upvals.is_empty() && !is_async;
        let hdr = GcHeader::native(upvals);
        let kind = if is_async {
            crate::vm::exec::native_call::NativeKind::Async
        } else {
            crate::vm::exec::native_call::NativeKind::of(builtin)
        };
        let nup = upvals.len();
        let upvals = LSlice::from_slice(self.mem(), upvals)?;
        let g = self.try_adopt(NativeClosure {
            hdr,
            f,
            upvals,
            is_async,
            kind,
            builtin,
        })?;
        // PUC 5.2+ makes a C function without upvalues a light value
        if nup > 0 || self.table_dialect == crate::runtime::table::Dialect::L51 {
            let size = self.sizes.c_closure(nup);
            self.acct_new(g.as_ptr() as *mut GcHeader, PucKind::CClosure, size);
        }
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
    // the instrumented PUC build takes its seed from `R38_SEED` too
    #[cfg(feature = "gc-trace")]
    if let Some(s) = std::env::var("R38_SEED").ok().and_then(|s| s.parse().ok()) {
        return s;
    }
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
