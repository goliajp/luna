//! String interning, native functions, and the objects the heap fixes.

use super::*;
use crate::runtime::mem::{LSlice, Oom, oom_abort};
use std::alloc::Layout;

mod native;

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
        // found again: no more a string only a load holds
        h.cls &= !parse::KEPT;
    }

    /// [`Heap::intern`], failing when memory runs out.
    #[inline]
    pub fn try_intern(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        if self.refuse_string_now(bytes.len()) {
            return Err(Oom(self.mem.mem()));
        }
        match self.intern_or_none(bytes) {
            Some(s) => Ok(s),
            None => Err(Oom(self.mem.mem())),
        }
    }

    /// [`Heap::try_intern`] with the failure as `None`: one register to
    /// return, where the `Result` takes two.
    pub(crate) fn intern_or_none(&mut self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        // 5.1 interns strings of every length
        if bytes.len() <= string::MAX_SHORT_LEN || self.rules.hash51() {
            let (p, is_new) = self
                .strings
                .intern(self.mem.mem(), bytes, self.seed, self.rules.hash51())
                .ok()?;
            // SAFETY: `StringTable::intern` made `p` (in its own chain only, which links no objects) or found it there
            let g = unsafe {
                if is_new {
                    self.link(p as *mut GcHeader);
                }
                Gc::from_ptr(p)
            };
            if !is_new {
                return Some(self.found_string(g, bytes.len()));
            }
            self.hook_string_made(g);
            self.acct_new_string(g, bytes.len());
            Some(g)
        } else {
            let p =
                string::alloc_long(self.mem.mem(), bytes, self.seed, self.rules.hash51()).ok()?;
            // SAFETY: `alloc_long` just allocated `p`, linked nowhere yet
            let g = unsafe {
                self.link(p as *mut GcHeader);
                Gc::from_ptr(p)
            };
            self.acct_new_string(g, bytes.len());
            Some(g)
        }
    }

    /// The one-byte string of `bytes` when it exists, found without a
    /// lookup (`string.sub` makes many).
    #[inline]
    pub(crate) fn find_single(&mut self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        let [c] = *bytes else { return None };
        let p = self.strings.single(c);
        // SAFETY: a string the table holds is one the heap manages
        (!p.is_null()).then(|| self.found_string(unsafe { Gc::from_ptr(p) }, 1))
    }

    /// An interned string `p` of `len` bytes found again.
    #[inline]
    fn found_string(&mut self, g: Gc<LuaStr>, len: usize) -> Gc<LuaStr> {
        if g.hdr.is_uncounted() {
            self.acct_promote(g, len);
        }
        let p = g.as_ptr();
        // PUC `luaS_new` resurrect guard (lstring.c).
        // The bucket-chain is walked open-loop without consulting GC
        // color; during incremental sweep an existing entry may be
        // dead-white (not swept yet, to be freed by the sweep). If we
        // hand its pointer back, the budget-paced sweep frees it out
        // from under the mutator and the next bucket walk dereferences
        // a libc-recycled slot.
        //
        // Flip the white bits to `current_white` so the upcoming sweep
        // skips it (PUC `changewhite`). Black / not-white objects are
        // already safe and untouched.
        // SAFETY: `p` comes from a handle, so its string is live; nothing
        // borrows its header while it is written
        unsafe {
            let f = (*(p as *mut GcHeader)).flags;
            if is_white(f) && (f & self.current_white) == 0 {
                self.resurrect(&mut *(p as *mut GcHeader));
                // the string sweep took it off already: PUC makes it again,
                // now or (found while nothing is counted) when the program
                // first uses it
                if (*p).hdr.sub & strt::SWEPT != 0 {
                    (*p).hdr.sub &= !strt::SWEPT;
                    if !self.counting() {
                        (*(p as *mut GcHeader)).set_uncounted(true);
                    } else {
                        self.acct_string(len);
                        self.strt_enter(g);
                    }
                }
            }
        }
        // found in the table and kept from this cycle's sweep by the
        // recoloring above
        g
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
        if self.rules.hash51() {
            // 5.1 looks every string up by its bytes
            let mut buf = [0u8; 256];
            if n <= buf.len() {
                buf[..a.len()].copy_from_slice(a);
                buf[a.len()..n].copy_from_slice(b);
                return self.try_intern(&buf[..n]);
            }
            return self.try_intern(&[a, b].concat());
        }
        let p = string::alloc_long_pair(self.mem.mem(), a, b, self.seed, self.rules.hash51())?;
        // SAFETY: `alloc_long_pair` just allocated `p`, linked nowhere yet
        let g = unsafe {
            self.link(p as *mut GcHeader);
            Gc::from_ptr(p)
        };
        self.acct_new_string(g, n);
        Ok(g)
    }

    /// The string of `parts` one after another (PUC `luaV_concat`'s
    /// result): looked up when short (5.1: always), else a new long string
    /// the parts are copied into.
    pub(crate) fn try_intern_parts(&mut self, parts: &[&[u8]]) -> Result<Gc<LuaStr>, Oom> {
        let n: usize = parts.iter().map(|p| p.len()).sum();
        if self.refuse_string_now(n) {
            return Err(Oom(self.mem.mem()));
        }
        if n <= string::MAX_SHORT_LEN {
            return self.try_intern(joined(parts, &mut [0u8; string::MAX_SHORT_LEN]));
        }
        // 5.1 looks every string up by its bytes
        if self.rules.hash51() && n <= 512 {
            return self.try_intern(joined(parts, &mut [0u8; 512]));
        }
        if self.rules.hash51() {
            return self.try_intern(&parts.concat());
        }
        let p = string::alloc_long_parts(self.mem.mem(), parts, self.seed)?;
        // SAFETY: `alloc_long_parts` just allocated `p`, linked nowhere
        // yet; linked, it is a live string of this heap
        let g = unsafe {
            self.link(p as *mut GcHeader);
            Gc::from_ptr(p)
        };
        self.acct_new_string(g, n);
        Ok(g)
    }

    /// 5.5 `lua_pushexternalstring` of `bytes` (longer than any short
    /// string), the block holding them handed over by a `luaL_Buffer`: a
    /// long string whose header alone `totalbytes` counts.
    pub(crate) fn new_external_string(&mut self, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        let p = string::alloc_long_parts(self.mem.mem(), &[bytes], self.seed)?;
        let silent = !self.counting();
        // SAFETY: `alloc_long_parts` just allocated `p`, linked nowhere
        // yet; linked, it is a live string of this heap, and only its kind
        // byte is written
        let g = unsafe {
            self.link(p as *mut GcHeader);
            if !silent {
                (*p).hdr.sub |= EXTERNAL;
            }
            Gc::from_ptr(p)
        };
        if silent {
            // SAFETY: `g` is the string linked above; nothing borrows its
            // header while its per-type byte is written
            unsafe { (*g.header()).set_uncounted(true) };
        } else {
            self.acct_obj(PucKind::LongStr, self.puc_size(PucObj::ExternalStr));
        }
        Ok(g)
    }

    /// The interned string with `bytes`, if one has been made; never
    /// allocates. A short name no string exists for is no table's key.
    pub fn find_interned(&self, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        if bytes.len() > string::MAX_SHORT_LEN && !self.rules.hash51() {
            return None;
        }
        let p = self.strings.find(bytes, self.seed, self.rules.hash51())?;
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

    /// PUC `luaC_fix` for the string `s`, wherever it is on `all` (one
    /// made before may be fixed again: nothing to do).
    pub(in crate::runtime) fn fix_string(&mut self, s: Gc<LuaStr>) {
        let h = s.as_ptr() as *mut GcHeader;
        let mut link: *mut *mut GcHeader = &mut self.all;
        // SAFETY: the links of `all` lead to live objects; no collection
        // is under way while a state is made
        unsafe {
            while !(*link).is_null() {
                if *link == h {
                    if self.young == h {
                        self.young = (*h).next;
                    }
                    *link = (*h).next;
                    (*h).next = self.fixed;
                    (*h).flags = (*h).with_slow((*h).flags & !COLOR_BITS);
                    self.fixed = h;
                    return;
                }
                link = &mut (**link).next;
            }
        }
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

/// `parts` one after another in `buf`, which has room for them.
fn joined<'b>(parts: &[&[u8]], buf: &'b mut [u8]) -> &'b [u8] {
    let mut at = 0;
    for p in parts {
        buf[at..at + p.len()].copy_from_slice(p);
        at += p.len();
    }
    &buf[..at]
}
