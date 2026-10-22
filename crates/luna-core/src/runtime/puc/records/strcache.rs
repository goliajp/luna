//! PUC's cache of the strings `luaS_new` makes from C strings (5.3 on):
//! `lua_pushstring`, `lua_getfield` and the like, and the parser's chunk
//! name, look the C string up by its address first, and a string found
//! there with the same characters is the one pushed. Only a long string
//! is affected (a short one is the interned string either way): found,
//! no new string is made.
//!
//! PUC takes a bucket by the address (address % 53, two strings to a
//! bucket). Two different addresses meeting in one bucket depend on where
//! the allocator put things, which changes from run to run; the cache is
//! kept here by the address alone, as where PUC's C strings live: the
//! characters of a Lua string, a buffer in a C function's frame (the
//! same at the same C call depth), or one fixed for the process.

use super::*;
use crate::runtime::mem::Oom;
use crate::runtime::table::Dialect;

/// Where a C string PUC hands to `luaS_new` lives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CStr {
    /// the characters of a Lua string, from byte `1` on
    In(*const GcHeader, u32),
    /// a buffer of C function `0` (a [`site`]) on C stack `1` (see
    /// `Vm::c_chain_key`), from byte `2` on
    Frame(u8, u32, u32),
    /// memory fixed for the process (the environment, a static table):
    /// kind `0`, entry `1`
    Fixed(u8, u32),
    /// the string of a name the scanner of the chunk whose source string is
    /// `0` made (one for each name in a chunk), by [`site::name_hash`]
    Named(*const GcHeader, u32),
}

/// The C functions and the fixed memory a [`CStr`] names.
pub(crate) mod site {
    /// `db_getinfo`'s `lua_Debug`, its `short_src`
    pub(crate) const GETINFO: u8 = 0;
    /// `searchpath`'s buffer of the path, cut into names
    pub(crate) const SEARCHPATH: u8 = 2;
    /// the environment's variables, by [`name_hash`]
    pub(crate) const GETENV: u8 = 3;

    /// A number for `name` (FNV-1a).
    pub(crate) fn name_hash(name: &[u8]) -> u32 {
        name.iter().fold(0x811c_9dc5_u32, |h, &b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        })
    }
}

/// The strings found at each address, the newer first.
#[derive(Default)]
pub(crate) struct StrCache {
    entries: Vec<(CStr, [Option<Gc<LuaStr>>; 2])>,
}

impl StrCache {
    /// The string at `at` with the characters `bytes`, if the cache has it.
    fn find(&self, at: CStr, bytes: &[u8]) -> Option<Gc<LuaStr>> {
        let (_, slots) = self.entries.iter().find(|(k, _)| *k == at)?;
        slots
            .iter()
            .flatten()
            .copied()
            .find(|s| s.as_bytes() == bytes)
    }

    /// `s`, made for `at`, goes in front of what `at` had.
    fn put(&mut self, at: CStr, s: Gc<LuaStr>) {
        match self.entries.iter_mut().find(|(k, _)| *k == at) {
            Some((_, slots)) => *slots = [Some(s), slots[0]],
            None => self.entries.push((at, [Some(s), None])),
        }
    }

    /// PUC `luaS_clearcache`, as marking ends: the dead strings go, and so
    /// does every address in a Lua string that died (its memory is
    /// another object's next).
    pub(in crate::runtime) fn clear_dead(&mut self) {
        // SAFETY: called as marking ends, before the sweep: every header
        // listed is still allocated, and only its flags are read
        let dead = |h: *const GcHeader| unsafe { is_white((*h).flags) };
        self.entries.retain_mut(|(k, slots)| {
            if let CStr::In(h, _) | CStr::Named(h, _) = *k
                && dead(h)
            {
                return false;
            }
            for s in slots.iter_mut() {
                if s.is_some_and(|s| dead(s.as_ptr() as *const GcHeader)) {
                    *s = None;
                }
            }
            slots.iter().any(Option::is_some)
        });
    }
}

impl Heap {
    /// PUC `luaS_new` of the C string at `at` holding `bytes`: from 5.3 a
    /// long string found in the cache at that address, else a new one,
    /// which the cache keeps there.
    pub(crate) fn intern_at(&mut self, at: CStr, bytes: &[u8]) -> Result<Gc<LuaStr>, Oom> {
        let cached = matches!(self.puc.dialect, Dialect::L53 | Dialect::L54 | Dialect::L55)
            && bytes.len() > string::MAX_SHORT_LEN;
        if let Some(s) = self.puc.strcache.find(at, bytes).filter(|_| cached) {
            return Ok(s);
        }
        let s = self.try_intern(bytes)?;
        if cached {
            self.puc.strcache.put(at, s);
        }
        Ok(s)
    }

    /// The parser's string of a chunk's name (`luaY_parser`'s `luaS_new`),
    /// `name` the Lua string whose characters it is, if any: that string
    /// itself where it is interned (5.1 interns every string), else a long
    /// string through the cache.
    pub(crate) fn chunk_name(
        &mut self,
        chunkname: &[u8],
        name: Option<Gc<LuaStr>>,
    ) -> Result<Gc<LuaStr>, Oom> {
        let long = chunkname.len() > string::MAX_SHORT_LEN && self.puc.dialect != Dialect::L51;
        match name {
            Some(n) if !long => Ok(n),
            Some(n) => self.intern_at(CStr::In(n.as_ptr() as *const GcHeader, 0), chunkname),
            None => self.try_intern(chunkname),
        }
    }
}
