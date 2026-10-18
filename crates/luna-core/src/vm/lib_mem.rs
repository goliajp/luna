//! Helpers the libraries share for memory errors.

use crate::vm::LuaError;

/// The end of opening a library at state creation. Opening runs before
/// anything can catch an error, so a memory error here ends the process,
/// as a standard library container would.
pub(crate) fn opened(r: Result<(), LuaError>) {
    if r.is_err() {
        crate::runtime::mem::oom_abort(std::alloc::Layout::new::<crate::runtime::Value>());
    }
}

use crate::runtime::mem::LVec;
use crate::runtime::table::Table;
use crate::runtime::value::NativeFn;
use crate::runtime::{Gc, Value};
use crate::version::LuaVersion;
use crate::vm::Vm;

impl Vm {
    /// A table with room for `n` fields, as PUC's `luaL_newlib` makes one:
    /// a library fills it without growing it.
    pub(crate) fn lib_table(&mut self, n: usize) -> Result<Gc<Table>, LuaError> {
        self.alloc_table_sized(0, n)
    }

    /// A native function without upvalues, collecting in an emergency when
    /// memory runs out.
    pub(crate) fn lib_native(&mut self, f: NativeFn) -> Result<Value, LuaError> {
        self.lib_native_with(f, &[])
    }

    /// A native function over `f` with `upvals`, collecting in an emergency
    /// when memory runs out.
    pub(crate) fn lib_native_with(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
    ) -> Result<Value, LuaError> {
        self.retry_oom(|vm| vm.heap.try_new_native(f, upvals, false))
            .map(Value::Native)
    }

    /// [`Vm::lib_native_with`] for a function PUC makes with `puc_nup`
    /// upvalues where luna keeps more.
    pub(crate) fn lib_native_puc(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        puc_nup: usize,
    ) -> Result<Value, LuaError> {
        // `lua_pushcclosure`, which checks for a collection with the
        // upvalues on the stack
        let base = self.lib_roots.len();
        self.lib_roots.extend_from_slice(upvals);
        self.api_check_before()?;
        let f = self
            .retry_oom(|vm| {
                vm.heap
                    .try_new_native_puc(f, upvals, crate::runtime::Builtin::None, puc_nup)
            })
            .map(Value::Native);
        self.lib_roots.truncate(base);
        let f = f?;
        self.api_check_after(f)?;
        Ok(f)
    }

    /// [`Vm::lib_native_with`] for a function standing for PUC's objects
    /// `objs` (see `Heap::try_new_native_stand_in`).
    pub(crate) fn lib_native_stand_in(
        &mut self,
        f: NativeFn,
        upvals: &[Value],
        objs: &[(crate::runtime::heap::PucKind, usize)],
        counted: usize,
    ) -> Result<Value, LuaError> {
        self.retry_oom(|vm| vm.heap.try_new_native_stand_in(f, upvals, objs, counted))
            .map(Value::Native)
    }

    /// A string of `parts` joined, composed in a block of the allocation
    /// context.
    pub(crate) fn concat_str(&mut self, parts: &[&[u8]]) -> Result<Value, LuaError> {
        let mut buf = crate::runtime::mem::LVec::new(self.heap.mem());
        for p in parts {
            buf.extend_from_slice(p)?;
        }
        self.str_value(&buf)
    }

    /// Room for `extra` more bytes in `buf`, a library's string buffer (PUC
    /// `luaL_Buffer`). When the allocation context refuses, each dialect
    /// fails as its buffer does: 5.1 pushes pieces as strings and 5.2 keeps
    /// the buffer in a userdata, both through the collector's allocation
    /// (5.2 with an emergency collection first); 5.3 to 5.5 call the
    /// allocation function directly, 5.3 raising "not enough memory for
    /// buffer allocation" and 5.4+ the memory error.
    #[inline]
    pub(crate) fn buf_reserve(&mut self, buf: &mut LVec<u8>, extra: usize) -> Result<(), LuaError> {
        if buf.capacity() - buf.len() >= extra {
            return Ok(());
        }
        match buf.reserve(extra) {
            Ok(()) => Ok(()),
            Err(_) => self.buf_refused(buf, extra),
        }
    }

    #[cold]
    #[inline(never)]
    fn buf_refused(&mut self, buf: &mut LVec<u8>, extra: usize) -> Result<(), LuaError> {
        match self.version() {
            LuaVersion::Lua52 => {
                if self.emergency_collect() && buf.reserve(extra).is_ok() {
                    return Ok(());
                }
                Err(self.mem_err())
            }
            LuaVersion::Lua53 => Err(crate::vm::builtins::raise_str(
                self,
                "not enough memory for buffer allocation",
            )),
            _ => Err(self.mem_err()),
        }
    }

    /// Append `bytes` to `buf`, a library's string buffer; see
    /// [`Vm::buf_reserve`].
    #[inline]
    pub(crate) fn buf_push(&mut self, buf: &mut LVec<u8>, bytes: &[u8]) -> Result<(), LuaError> {
        self.buf_reserve(buf, bytes.len())?;
        buf.extend_from_slice(bytes).map_err(LuaError::from)
    }

    /// Return `n` results produced by `vals`, written straight into the
    /// result slots (no temporary vector of them).
    pub(crate) fn nat_ret_iter(
        &mut self,
        fs: u32,
        n: usize,
        vals: impl IntoIterator<Item = Value>,
    ) -> Result<u32, LuaError> {
        let base = fs as usize;
        self.grow_stack(base + n)?;
        let mut k = 0;
        for v in vals.into_iter().take(n) {
            self.stack[base + k] = v;
            k += 1;
        }
        Ok(k as u32)
    }

    /// PUC's `LUAL_BUFFERSIZE`, the room a library buffer has on the C
    /// stack before it allocates: the C library's `BUFSIZ` up to 5.2,
    /// `0x80 * sizeof(void *) * sizeof(lua_Integer)` in 5.3 and
    /// `16 * sizeof(void *) * sizeof(lua_Number)` from 5.4 on.
    pub(crate) fn lual_buffersize(v: LuaVersion) -> usize {
        let ptr = std::mem::size_of::<usize>();
        match v {
            LuaVersion::Lua51 | LuaVersion::Lua52 => crate::vm::lib_io::BUFSIZ,
            LuaVersion::Lua53 => 0x80 * ptr * 8,
            _ => 16 * ptr * 8,
        }
    }

    /// An empty string buffer for a library function: the one the last
    /// call gave back, when there is one. The Vm starts with one of
    /// `LUAL_BUFFERSIZE`, so a buffer that fits PUC's C stack allocates
    /// nothing, as there.
    #[inline]
    pub(crate) fn new_buf(&mut self) -> LVec<u8> {
        let mem = self.heap.mem();
        std::mem::replace(&mut self.buf_spare, LVec::new(mem))
    }

    /// Keep `buf` for the next [`Vm::new_buf`], unless it has grown large.
    #[inline]
    pub(crate) fn recycle_buf(&mut self, mut buf: LVec<u8>) {
        if buf.capacity() <= 8192 && buf.capacity() >= self.buf_spare.capacity() {
            buf.clear();
            self.buf_spare = buf;
        }
    }

    /// `t[name] = v` with a raw store, as a library fills its tables.
    pub(crate) fn lib_set(&mut self, t: Gc<Table>, name: &str, v: Value) -> Result<(), LuaError> {
        let k = self.str_value(name.as_bytes())?;
        self.lib_set_value(t, k, v)
    }

    /// `t[k] = v` with a raw store that ignores the read-only mark, as a
    /// library fills a table it made.
    pub(crate) fn lib_set_value(
        &mut self,
        t: Gc<Table>,
        k: Value,
        v: Value,
    ) -> Result<(), LuaError> {
        // SAFETY: `t` is a table the caller holds; each borrow lives for one
        // `set`, which touches only the heap and the table and does not
        // collect
        let r = self.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, k, v));
        r.map_err(|e| self.table_error(e))?;
        self.heap.barrier_store(t, &[k, v]);
        Ok(())
    }

    /// The global `name` set to `v`, as a library registers itself.
    pub(crate) fn lib_set_global(&mut self, name: &str, v: Value) -> Result<(), LuaError> {
        let g = self.globals();
        self.lib_set(g, name, v)
    }

    /// A userdata with `payload`, collecting in an emergency when memory
    /// runs out. The object is made empty, so a second attempt has nothing
    /// to rebuild, and then given the payload.
    /// `size` is what PUC's userdata of the kind takes.
    pub(crate) fn lib_userdata(
        &mut self,
        payload: crate::runtime::userdata::UserdataPayload,
        writable: bool,
        size: usize,
    ) -> Result<Gc<crate::runtime::Userdata>, LuaError> {
        use crate::runtime::userdata::UserdataPayload;
        let u = self.retry_oom(|vm| {
            vm.heap
                .try_new_userdata_sized(UserdataPayload::Empty, writable, size)
        })?;
        // SAFETY: `u` was made above and no other handle to it exists yet; the
        // borrow covers one store, which allocates nothing
        unsafe { u.as_mut() }.payload = payload;
        Ok(u)
    }

    /// A closed upvalue holding `v`, collecting in an emergency when memory
    /// runs out.
    pub(crate) fn lib_closed_upvalue(
        &mut self,
        v: Value,
    ) -> Result<Gc<crate::runtime::Upvalue>, LuaError> {
        self.retry_oom(|vm| {
            vm.heap
                .try_new_upvalue(crate::runtime::UpvalState::Closed(v))
        })
    }
}
