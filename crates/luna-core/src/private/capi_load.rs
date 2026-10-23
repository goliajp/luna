//! The C API's view of a Vm: loading, dumping, opening libraries, the collector and warnings.

use crate::runtime::*;
use crate::vm::LuaError;
use crate::vm::exec::host_c::*;

/// What the C API (`luna-jit`'s `capi`) asks of a Vm: loading, dumping, opening libraries, the collector and warnings.
pub trait VmCApiLoad {
    /// Whether `prefix`, the start of a binary chunk a `lua_load` reader
    /// delivered, already holds every byte PUC's undumper reads: PUC stops
    /// calling the reader there.
    fn host_binary_chunk_complete(&mut self, prefix: &[u8]) -> bool;
    /// Compile or undump `src` as the chunk `chunkname` (PUC `f_parser`):
    /// the function and the number of upvalues its prototype declares, or
    /// the error.
    fn host_load_chunk(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError>;
    /// `Vm::host_load_chunk` for a text chunk a reader handed over
    /// while it was parsed (see `Vm::text_load`).
    fn host_load_parsed(
        &mut self,
        parsed: crate::vm::exec::ParsedText,
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError>;
    /// Set the first upvalue of a function `lua_load` just made, which no
    /// other code holds yet, to `v` (PUC `lua_load`: the globals), in the
    /// upvalue the load made. Never fails.
    ///
    /// # Safety
    /// `cl` is a function a load (`host_load_chunk`, `host_load_parsed`) just made, which nothing else holds yet: its first upvalue is its own and closed, so setting it changes no other function and no stack slot.
    unsafe fn host_set_first_upvalue(
        &mut self,
        cl: Gc<LuaClosure>,
        v: Value,
    ) -> Result<(), LuaError>;
    /// The binary chunk `lua_dump` writes for `f`: PUC bytecode of the
    /// dialect, as `string.dump` writes it, or luna's own format for a
    /// function the dialect's instruction set cannot hold (as one block),
    /// with the size of each block PUC's dumper writes on its own; `None`
    /// for a value that is not a Lua function.
    fn host_dump(&self, f: Value, strip: bool) -> Option<(Vec<u8>, Vec<usize>)>;
    /// Start the collector in incremental mode, as a state made by
    /// `lua_newstate` is (the stand-alone interpreter switches 5.4 and 5.5
    /// to generational mode, which is a fresh Vm's mode).
    fn host_gc_start_incremental(&mut self);
    /// Turn the default warning function on (5.5's `luaL_newstate`).
    fn host_warn_on(&mut self);
    /// PUC `lua_warning`: one piece of a warning, to the warning function.
    fn host_warning(&mut self, msg: &[u8], to_cont: bool) -> Result<(), LuaError>;
    /// PUC `lua_gc` with the option `what` of the dialect's numbering and
    /// its arguments (`args`: those the option takes, in order, the rest
    /// zero), mapped onto luna's collector the way `collectgarbage` is.
    /// 5.1 to 5.3 raise a finalizer's error from a full collection.
    fn host_gc(&mut self, what: i32, args: [i64; 3]) -> Result<i32, LuaError>;
    /// Open the standard library `name` (`_G` for the base library) as the
    /// dialect's `luaopen_*` does, and return what that function returns.
    fn host_open_lib(&mut self, name: &str) -> Result<Vec<Value>, HostOpenError>;
}

impl VmCApiLoad for crate::vm::Vm {
    #[inline]
    fn host_binary_chunk_complete(&mut self, prefix: &[u8]) -> bool {
        crate::vm::Vm::host_binary_chunk_complete(self, prefix)
    }
    #[inline]
    fn host_load_chunk(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError> {
        crate::vm::Vm::host_load_chunk(self, src, chunkname)
    }
    #[inline]
    fn host_load_parsed(
        &mut self,
        parsed: crate::vm::exec::ParsedText,
        chunkname: &[u8],
    ) -> Result<(Gc<LuaClosure>, usize), HostLoadError> {
        crate::vm::Vm::host_load_parsed(self, parsed, chunkname)
    }
    #[inline]
    unsafe fn host_set_first_upvalue(
        &mut self,
        cl: Gc<LuaClosure>,
        v: Value,
    ) -> Result<(), LuaError> {
        crate::vm::Vm::host_set_first_upvalue(self, cl, v)
    }
    #[inline]
    fn host_dump(&self, f: Value, strip: bool) -> Option<(Vec<u8>, Vec<usize>)> {
        crate::vm::Vm::host_dump(self, f, strip)
    }
    #[inline]
    fn host_gc_start_incremental(&mut self) {
        crate::vm::Vm::host_gc_start_incremental(self)
    }
    #[inline]
    fn host_warn_on(&mut self) {
        crate::vm::Vm::host_warn_on(self)
    }
    #[inline]
    fn host_warning(&mut self, msg: &[u8], to_cont: bool) -> Result<(), LuaError> {
        crate::vm::Vm::host_warning(self, msg, to_cont)
    }
    #[inline]
    fn host_gc(&mut self, what: i32, args: [i64; 3]) -> Result<i32, LuaError> {
        crate::vm::Vm::host_gc(self, what, args)
    }
    #[inline]
    fn host_open_lib(&mut self, name: &str) -> Result<Vec<Value>, HostOpenError> {
        crate::vm::Vm::host_open_lib(self, name)
    }
}
