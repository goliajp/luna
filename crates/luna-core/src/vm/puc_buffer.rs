//! PUC's `luaL_Buffer`, as far as what it allocates goes. A library
//! function builds its result in a Rust buffer as before and tells the
//! model what PUC's C code would do with its buffer at each step; the model
//! makes the strings, boxes and metatable PUC makes, in its order:
//!
//! - 5.1: the C buffer holds `LUAL_BUFFERSIZE` bytes; each time it fills
//!   its contents become a string on the stack, and the strings there are
//!   joined when there are too many or a new one is longer than the one
//!   below (`adjuststack`)
//! - 5.2: a bigger buffer is a new full userdata each time, the old one
//!   left as garbage
//! - 5.3 to 5.5: one box userdata, whose block the allocation function
//!   gives (seen by the accounting trace, not by `totalbytes`), with a
//!   metatable kept in the registry; 5.5 hands the block to the result
//!   string

use crate::runtime::dialect::Dialect;
use crate::runtime::puc::PucObj;
use crate::runtime::{Gc, LuaStr, Userdata, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod finish;
mod stack51;

/// A `luaL_Buffer` of the running native.
pub(crate) struct PucBuffer {
    d: Dialect,
    /// `B->size`
    size: usize,
    /// `B->n` (5.1: the bytes in the C buffer)
    n: usize,
    /// a box (5.2: a userdata) holds the bytes
    boxed: bool,
    /// 5.3+: the box
    ubox: Option<Gc<Userdata>>,
    /// the running native's top when the buffer began, the box's slot
    slot: Option<u32>,
    /// 5.1: where the C buffer's bytes start in the content
    start: usize,
    /// 5.1: where this buffer's strings start in `Vm::lib_roots`
    base: crate::vm::exec::LibMark,
}

impl PucBuffer {
    /// `luaL_buffinit`
    #[inline]
    pub(crate) fn new(vm: &mut Vm) -> PucBuffer {
        PucBuffer {
            d: vm.heap.dialect(),
            size: Vm::lual_buffersize(vm.version()),
            n: 0,
            boxed: false,
            ubox: None,
            slot: vm.native_top(),
            start: 0,
            base: vm.lib_roots.mark(),
        }
    }

    /// [`PucBuffer::new`] in code built for the rule `PRE` (see
    /// `vm/rules.rs`): 5.1 and 5.2 make no box, so need no slot for one.
    #[inline]
    pub(crate) fn new_by<const PRE: bool>(vm: &mut Vm) -> PucBuffer {
        if !PRE {
            return PucBuffer::new(vm);
        }
        PucBuffer {
            d: vm.heap.dialect(),
            size: crate::vm::lib_io::BUFSIZ,
            n: 0,
            boxed: false,
            ubox: None,
            slot: None,
            start: 0,
            base: vm.lib_roots.mark(),
        }
    }

    /// `luaL_buffinitsize(sz)` (5.2+)
    pub(crate) fn with_size(vm: &mut Vm, sz: usize) -> Result<PucBuffer, LuaError> {
        let mut b = PucBuffer::new(vm);
        b.prep(vm, &[], sz)?;
        Ok(b)
    }

    /// `l` bytes added at the end of `out`, the content so far
    /// (`luaL_addlstring`, `luaL_addchar`)
    #[inline]
    pub(crate) fn add(&mut self, vm: &mut Vm, out: &[u8], l: usize) -> Result<(), LuaError> {
        if self.size - self.n >= l {
            self.n += l;
            return Ok(());
        }
        self.add_growing(vm, out, l)
    }

    #[cold]
    #[inline(never)]
    fn add_growing(&mut self, vm: &mut Vm, out: &[u8], l: usize) -> Result<(), LuaError> {
        if self.d == Dialect::L51 {
            // one character at a time: the buffer is emptied when a
            // character finds it full
            let mut l = l;
            while l > 0 {
                if self.n == self.size {
                    self.prepbuffer_51(vm, out)?;
                }
                let take = (self.size - self.n).min(l);
                self.n += take;
                l -= take;
            }
            return Ok(());
        }
        if l > 0 || self.d == Dialect::L52 {
            self.prep(vm, out, l)?;
        }
        self.n += l;
        Ok(())
    }

    /// `k` bytes added at the end of `out` a character at a time
    /// (`luaL_addchar`): the buffer grows each time a character finds it
    /// full.
    #[inline]
    pub(crate) fn add_chars(&mut self, vm: &mut Vm, out: &[u8], k: usize) -> Result<(), LuaError> {
        // with room for them all, no dialect empties or grows the buffer
        if self.size - self.n >= k {
            self.n += k;
            return Ok(());
        }
        self.add_chars_growing(vm, out, k)
    }

    #[cold]
    #[inline(never)]
    fn add_chars_growing(&mut self, vm: &mut Vm, out: &[u8], k: usize) -> Result<(), LuaError> {
        if self.d == Dialect::L51 {
            return self.add(vm, out, k);
        }
        let mut k = k;
        while k > 0 {
            if self.n == self.size {
                self.prep(vm, out, 1)?;
            }
            let take = (self.size - self.n).min(k);
            self.n += take;
            k -= take;
        }
        Ok(())
    }

    /// The last `n` bytes taken off (`luaL_buffsub`).
    pub(crate) fn sub(&mut self, n: usize) {
        self.n -= n;
    }

    /// `sz` bytes written into the room asked for before (`luaL_addsize`).
    pub(crate) fn add_size(&mut self, sz: usize) {
        self.n += sz;
    }

    /// Room for `sz` more bytes (`luaL_prepbuffsize`; 5.1's
    /// `luaL_prepbuffer`, which empties the buffer whatever it holds).
    #[inline]
    pub(crate) fn prep(&mut self, vm: &mut Vm, out: &[u8], sz: usize) -> Result<(), LuaError> {
        if self.d == Dialect::L51 {
            return self.prepbuffer_51(vm, out);
        }
        if self.size - self.n >= sz {
            return Ok(());
        }
        self.grow(vm, sz)
    }

    /// [`PucBuffer::prep`] when the room is short: a bigger buffer.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, vm: &mut Vm, sz: usize) -> Result<(), LuaError> {
        let newsize = match self.d {
            Dialect::L52 | Dialect::L53 => (self.size * 2).max(self.n + sz),
            Dialect::L54 => (self.size / 2 * 3).max(self.n + sz),
            _ => (self.size + self.size / 2).max(self.n + sz + 1),
        };
        let old = self.boxed.then_some(self.size);
        vm.refuse_box(&mut self.ubox, self.slot, self.d, (old, newsize))?;
        match self.d {
            Dialect::L52 => {
                vm.api_check_before::<true>()?;
                vm.ghost(PucObj::udata(0, newsize))?;
            }
            _ if self.boxed => vm.heap.trace_raw(self.size, newsize),
            _ => {
                self.ubox = Some(vm.new_box(self.slot)?);
                vm.heap.trace_raw(0, newsize);
            }
        }
        vm.box_block(self.ubox, newsize);
        self.size = newsize;
        self.boxed = true;
        Ok(())
    }

    /// `luaL_addvalue` of `s`, whose bytes the caller added to `out`.
    pub(crate) fn add_value(
        &mut self,
        vm: &mut Vm,
        out: &[u8],
        s: Gc<LuaStr>,
    ) -> Result<(), LuaError> {
        let vl = s.len();
        // the value is on the stack while the buffer makes room
        let at = vm.lib_roots.mark();
        vm.lib_roots.push(Value::Str(s));
        if self.d != Dialect::L51 {
            let r = self.add(vm, out, vl);
            vm.lib_roots.release(at);
            return r;
        }
        if vl <= self.size - self.n {
            vm.lib_roots.release(at);
            self.n += vl;
            return Ok(());
        }
        // it stays there as one more string, the buffer's contents put
        // below it (`lua_insert`)
        let end = out.len() - vl;
        if self.empty_51(vm, &out[..end])? {
            vm.lib_roots.swap(at.at(), at.at() + 1);
        }
        self.start = out.len();
        self.adjust_51(vm)
    }
}

impl Vm {
    /// The string of `out` a library function builds in one buffer: 5.1
    /// adds it a character at a time (`luaL_addchar`, `luaL_addlstring`),
    /// later dialects ask for its whole size first (`luaL_buffinitsize`,
    /// `luaL_pushresultsize`).
    pub(crate) fn buffer_result<const PRE: bool>(&mut self, out: &[u8]) -> Result<Value, LuaError> {
        self.lib_scope(|vm| {
            let mut b = match vm.heap.dialect() {
                Dialect::L51 => PucBuffer::new(vm),
                _ => PucBuffer::with_size(vm, out.len())?,
            };
            match vm.heap.dialect() {
                Dialect::L51 => b.add(vm, out, out.len())?,
                _ => b.n = out.len(),
            }
            Ok(Value::Str(b.finish::<PRE>(vm, out)?))
        })
    }
}
