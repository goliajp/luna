//! The interned short strings: PUC's string table, chained by hash.

use super::*;

/// Open hashing with per-string chains (PUC stringtable shape).
pub(crate) struct StringTable {
    buckets: LVec<*mut LuaStr>,
    count: usize,
    /// a table too full that cannot grow fails the new string (PUC up to
    /// 5.3, whose `luaS_resize` raises); else it keeps working
    strict: bool,
}

impl StringTable {
    pub(crate) fn new(mem: MemRef) -> Result<StringTable, Oom> {
        let mut buckets = LVec::new(mem);
        buckets.resize(64, ptr::null_mut())?;
        Ok(StringTable {
            buckets,
            count: 0,
            strict: false,
        })
    }

    /// Find or create an interned short string. Returns `(ptr, newly_created)`.
    #[inline]
    pub(crate) fn intern(
        &mut self,
        mem: MemRef,
        bytes: &[u8],
        seed: u32,
        h51: bool,
    ) -> Result<(*mut LuaStr, bool), Oom> {
        debug_assert!(bytes.len() <= MAX_SHORT_LEN);
        let h = if h51 {
            lua_hash_51(bytes)
        } else {
            lua_hash(bytes, seed)
        };
        let b = h as usize & (self.buckets.len() - 1);
        let mut cur = self.buckets[b];
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe {
            while !cur.is_null() {
                // the hash first: it is in the header already loaded for
                // `hnext`, and keeps the byte compare to the string sought
                // (`bytes_of` compares lengths before bytes)
                if (*cur).hash.get() == h && bytes_of(cur) == bytes {
                    return Ok((cur, false));
                }
                cur = (*cur).hnext;
            }
        }
        if self.count >= self.buckets.len() && !self.grow() && self.strict {
            return Err(Oom(mem));
        }
        let b = h as usize & (self.buckets.len() - 1);
        let p = alloc_str(mem, bytes, true, h, true)?;
        // SAFETY: `p` was just returned by `alloc_str` and is not in any chain yet; nothing else points at it
        unsafe {
            (*p).hnext = self.buckets[b];
        }
        self.buckets[b] = p;
        self.count += 1;
        Ok((p, true))
    }

    /// The interned string with `bytes` (at most `MAX_SHORT_LEN` long), if
    /// any.
    pub(crate) fn find(&self, bytes: &[u8], seed: u32, h51: bool) -> Option<*mut LuaStr> {
        let h = if h51 {
            lua_hash_51(bytes)
        } else {
            lua_hash(bytes, seed)
        };
        let mut cur = self.buckets[h as usize & (self.buckets.len() - 1)];
        // SAFETY: the bucket chains hold only interned strings that are still allocated
        unsafe {
            while !cur.is_null() {
                if (*cur).hdr.aux as usize == bytes.len() && bytes_of(cur) == bytes {
                    return Some(cur);
                }
                cur = (*cur).hnext;
            }
        }
        None
    }

    #[cold]
    #[inline(never)]
    fn grow(&mut self) -> bool {
        // false, the table as it was, when it cannot grow
        let mut nb = LVec::new(self.buckets.mem());
        if nb.resize(self.buckets.len() * 2, ptr::null_mut()).is_err() {
            return false;
        }
        let mask = nb.len() - 1;
        for &head in self.buckets.iter() {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: the bucket chains hold only interned strings that are still allocated: `remove` unlinks a string before the sweep frees it
                unsafe {
                    let next = (*cur).hnext;
                    let b = (*cur).hash.get() as usize & mask;
                    (*cur).hnext = nb[b];
                    nb[b] = cur;
                    cur = next;
                }
            }
        }
        self.buckets = nb;
        true
    }

    /// Hash every interned string again, by PUC 5.1's function when `h51`
    /// and the seeded one otherwise, and chain it in its new bucket: for
    /// the few strings a heap makes before its dialect is known.
    pub(crate) fn rehash(&mut self, seed: u32, h51: bool, strict: bool) {
        self.strict = strict;
        let mut all = ptr::null_mut::<LuaStr>();
        for head in self.buckets.iter_mut() {
            let mut cur = std::mem::replace(head, ptr::null_mut());
            while !cur.is_null() {
                // SAFETY: the bucket chains hold only interned strings that are still allocated (see `grow`)
                unsafe {
                    let next = (*cur).hnext;
                    (*cur).hnext = all;
                    all = cur;
                    cur = next;
                }
            }
        }
        let mask = self.buckets.len() - 1;
        while !all.is_null() {
            // SAFETY: as above; each string is moved from `all` to one chain
            unsafe {
                let next = (*all).hnext;
                let b = bytes_of(all);
                let h = if h51 {
                    lua_hash_51(b)
                } else {
                    lua_hash(b, seed)
                };
                (*all).hash.set(h);
                let i = h as usize & mask;
                (*all).hnext = self.buckets[i];
                self.buckets[i] = all;
                all = next;
            }
        }
    }

    /// Unlink a dying interned string (called from sweep).
    ///
    /// # Safety
    /// `p` is a short string this table interned, still allocated and in its
    /// bucket.
    pub(crate) unsafe fn remove(&mut self, p: *mut LuaStr) {
        // SAFETY: `p` is allocated and chained (the caller's contract), and every other chained string is allocated too, so `cur` always points at a bucket slot or at the `hnext` of a live string
        unsafe {
            let b = (*p).hash.get() as usize & (self.buckets.len() - 1);
            let mut cur: *mut *mut LuaStr = &mut self.buckets[b];
            while !(*cur).is_null() {
                if *cur == p {
                    *cur = (*p).hnext;
                    self.count -= 1;
                    return;
                }
                cur = &mut (**cur).hnext;
            }
            unreachable!("interned string missing from string table");
        }
    }
}
