//! What the io library finds where PUC's finds it: the files' metatable
//! in the registry (`LUA_FILEHANDLE`), and the default files in the
//! registry (5.2 on) or in the library's environment (5.1). Nothing is
//! kept beside them, so a collection sees what PUC's does.

use super::*;
use crate::runtime::Table;

/// The registry's field `name`, read raw without making the name.
fn registry_field(vm: &Vm, name: &[u8]) -> Value {
    match (vm.registry, vm.heap.find_interned(name)) {
        (Some(r), Some(k)) => r.get(Value::Str(k)),
        _ => Value::Nil,
    }
}

/// The files' metatable (`luaL_getmetatable(L, LUA_FILEHANDLE)`).
pub(crate) fn file_metatable(vm: &Vm) -> Option<Gc<Table>> {
    match registry_field(vm, b"FILE*") {
        Value::Table(t) => Some(t),
        _ => None,
    }
}

/// The registry's name of the default file `which` (5.2 on).
fn io_key(which: Io) -> &'static [u8] {
    match which {
        Io::Input => b"_IO_input",
        Io::Output => b"_IO_output",
    }
}

/// The default file `which`, if a file is there.
pub(super) fn default_file(vm: &Vm, which: Io) -> Option<Gc<Userdata>> {
    if vm.version() == LuaVersion::Lua51 {
        return vm.heap.env51_io(which as usize);
    }
    match registry_field(vm, io_key(which)) {
        Value::Userdata(u) => Some(u),
        _ => None,
    }
}

/// The default file `which` set to `u`: 5.1 `lua_rawseti` into the
/// environment, later `lua_setfield` into the registry.
pub(super) fn set_default_file(vm: &mut Vm, which: Io, u: Gc<Userdata>) -> Result<(), LuaError> {
    if vm.version() == LuaVersion::Lua51 {
        vm.heap.env51_set_io(which as usize, u);
        return Ok(());
    }
    let reg = vm.host_registry();
    let k = vm.str_value(io_key(which))?;
    vm.host_set_index(Value::Table(reg), k, Value::Userdata(u))
}

impl Vm {
    /// `io.stdin` as the io library made it, whatever the script later
    /// does to the `io` table, while it lives: the stream a host's line
    /// reads share.
    pub(crate) fn io_stdin(&self) -> Option<Gc<Userdata>> {
        let h = self.heap.stdin_ud();
        // SAFETY: the heap clears it when it frees the userdata
        (!h.is_null()).then(|| unsafe { Gc::from_ptr(h as *mut Userdata) })
    }
}
