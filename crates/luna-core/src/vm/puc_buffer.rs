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

use crate::runtime::heap::PucKind;
use crate::runtime::table::Dialect;
use crate::runtime::{Gc, LuaStr, Userdata, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// 5.1 `adjuststack`'s `LIMIT` (`LUA_MINSTACK / 2`).
const LIMIT_51: usize = 10;

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
    /// 5.1: where the C buffer's bytes start in the content
    start: usize,
    /// 5.1: where this buffer's strings start in `Vm::lib_roots`
    base: usize,
}

impl PucBuffer {
    /// `luaL_buffinit`
    #[inline]
    pub(crate) fn new(vm: &mut Vm) -> PucBuffer {
        PucBuffer {
            d: vm.heap.table_dialect,
            size: Vm::lual_buffersize(vm.version()),
            n: 0,
            boxed: false,
            ubox: None,
            start: 0,
            base: vm.lib_roots.len(),
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
        match self.d {
            Dialect::L52 => {
                vm.api_check_before();
                let size = vm.heap.sizes.userdata(0, newsize);
                vm.ghost(PucKind::Userdata, size)?;
            }
            _ if self.boxed => vm.heap.trace_raw(self.size, newsize),
            _ => {
                self.ubox = Some(vm.new_box()?);
                vm.heap.trace_raw(0, newsize);
            }
        }
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
        let at = vm.lib_roots.len();
        vm.lib_roots.push(Value::Str(s));
        if self.d != Dialect::L51 {
            let r = self.add(vm, out, vl);
            vm.lib_roots.truncate(at);
            return r;
        }
        if vl <= self.size - self.n {
            vm.lib_roots.truncate(at);
            self.n += vl;
            return Ok(());
        }
        // it stays there as one more string, the buffer's contents put
        // below it (`lua_insert`)
        let end = out.len() - vl;
        if self.empty_51(vm, &out[..end])? {
            vm.lib_roots.swap(at, at + 1);
        }
        self.start = out.len();
        self.adjust_51(vm)
    }

    /// `luaL_pushresult`: the string of `out`.
    pub(crate) fn finish(mut self, vm: &mut Vm, out: &[u8]) -> Result<Gc<LuaStr>, LuaError> {
        match self.d {
            Dialect::L51 => {
                self.empty_51(vm, out)?;
                let n = vm.lib_roots.len() - self.base;
                let r = if n == 0 {
                    vm.alloc_str(b"")?
                } else {
                    if n >= 2 {
                        self.concat_51(vm, n)?;
                    }
                    match vm.lib_roots[self.base] {
                        Value::Str(s) => s,
                        _ => unreachable!("the buffer's strings"),
                    }
                };
                vm.lib_roots.truncate(self.base);
                Ok(r)
            }
            Dialect::L55 if self.boxed => {
                // the box's block becomes the string's
                if self.size != out.len() + 1 {
                    vm.heap.trace_raw(self.size, out.len() + 1);
                }
                let r = vm.retry_oom(|vm| vm.heap.new_external_string(out))?;
                vm.api_check_after(Value::Str(r));
                vm.box_closed();
                // `lua_gc(L, LUA_GCSTEP, len)`, the string and the box on
                // the stack
                vm.lib_roots.push(Value::Str(r));
                vm.gc_top = vm.native_top().unwrap_or_else(|| vm.lua_window_end());
                vm.gc_lua_step(out.len() as i64);
                vm.lib_roots.pop();
                self.drop_box(vm);
                Ok(r)
            }
            _ => {
                vm.api_check_before();
                let r = vm.alloc_str(out)?;
                vm.api_check_after(Value::Str(r));
                if self.boxed && self.d >= Dialect::L53 {
                    if self.d >= Dialect::L54 {
                        // `lua_closeslot` calls the box's `__close`
                        vm.box_closed();
                    }
                    vm.heap.trace_raw(self.size, 0);
                    self.drop_box(vm);
                }
                Ok(r)
            }
        }
    }

    fn drop_box(&mut self, vm: &mut Vm) {
        if let Some(u) = self.ubox.take() {
            vm.drop_box(u);
        }
    }

    /// 5.1 `luaL_prepbuffer`
    fn prepbuffer_51(&mut self, vm: &mut Vm, out: &[u8]) -> Result<(), LuaError> {
        let end = self.start + self.n;
        if self.empty_51(vm, &out[..end])? {
            self.adjust_51(vm)?;
        }
        Ok(())
    }

    /// 5.1 `emptybuffer`: the C buffer's bytes, the end of `out`, become a
    /// string on the stack.
    fn empty_51(&mut self, vm: &mut Vm, out: &[u8]) -> Result<bool, LuaError> {
        if self.n == 0 {
            return Ok(false);
        }
        vm.api_check_before();
        let s = vm.alloc_str(&out[self.start..self.start + self.n])?;
        vm.lib_roots.push(Value::Str(s));
        self.start += self.n;
        self.n = 0;
        Ok(true)
    }

    /// 5.1 `adjuststack`
    fn adjust_51(&mut self, vm: &mut Vm) -> Result<(), LuaError> {
        let lvl = vm.lib_roots.len() - self.base;
        if lvl <= 1 {
            return Ok(());
        }
        let len = |vm: &Vm, i: usize| match vm.lib_roots[vm.lib_roots.len() - i] {
            Value::Str(s) => s.len(),
            _ => unreachable!("the buffer's strings"),
        };
        let mut toget = 1;
        let mut toplen = len(vm, 1);
        loop {
            let l = len(vm, toget + 1);
            if lvl - toget + 1 >= LIMIT_51 || toplen > l {
                toplen += l;
                toget += 1;
            } else {
                break;
            }
            if toget >= lvl {
                break;
            }
        }
        self.concat_51(vm, toget)
    }

    /// `lua_concat(L, n)` of the top `n` strings of the buffer
    fn concat_51(&mut self, vm: &mut Vm, n: usize) -> Result<(), LuaError> {
        let from = vm.lib_roots.len() - n;
        let mut st: Vec<Value> = vm.lib_roots[from..].to_vec();
        vm.api_check_before();
        vm.concat_pieces(&mut st, n)?;
        vm.lib_roots.truncate(from);
        vm.lib_roots.push(st[0]);
        Ok(())
    }
}

impl Vm {
    /// The string of `out` a library function builds in one buffer: 5.1
    /// adds it a character at a time (`luaL_addchar`, `luaL_addlstring`),
    /// later dialects ask for its whole size first (`luaL_buffinitsize`,
    /// `luaL_pushresultsize`).
    pub(crate) fn buffer_result(&mut self, out: &[u8]) -> Result<Value, LuaError> {
        let b = if self.heap.table_dialect == Dialect::L51 {
            let mut b = PucBuffer::new(self);
            b.add(self, out, out.len())?;
            b
        } else {
            let mut b = PucBuffer::with_size(self, out.len())?;
            b.n = out.len();
            b
        };
        Ok(Value::Str(b.finish(self, out)?))
    }
}
