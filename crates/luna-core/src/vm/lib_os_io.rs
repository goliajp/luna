//! The os library, plus the entry point that opens io and os together and
//! the base functions that read files (`loadfile`, `dofile`). Shaped per
//! dialect after loslib.c 5.1–5.5.
//!
//! Time is UTC throughout: luna-core links no libc, so it has no time zone
//! database. `os.date` without `!` and `os.time` read and write UTC broken-
//! down time, which keeps `os.time(os.date("*t", t)) == t` exact.

use crate::runtime::{Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::lib_io;
use std::io::Read;

mod calendar;
mod date;
mod load;
mod system;
mod time;
use calendar::*;
use date::*;
pub(crate) use load::load_chunk;
pub(crate) use load::load_path;
pub(crate) use load::load_stdin_text;
pub(crate) use load::nat_loadfile;
use load::*;
use system::*;
use time::*;

/// Open the `os` and `io` libraries, failing when memory runs out.
pub(crate) fn try_open_os_io(vm: &mut Vm) -> Result<(), LuaError> {
    try_open_os(vm)?;
    lib_io::try_open_io(vm)?;
    try_open_file_loaders(vm)
}

/// [`open_os`], failing when memory runs out.
pub(crate) fn try_open_os(vm: &mut Vm) -> Result<(), LuaError> {
    let os = vm.lib_table(11)?;
    for (name, f) in [
        ("clock", os_clock as crate::runtime::value::NativeFn),
        ("date", os_date),
        ("difftime", os_difftime),
        ("execute", os_execute),
        ("exit", os_exit),
        ("getenv", os_getenv),
        ("remove", os_remove),
        ("rename", os_rename),
        ("time", os_time),
        ("tmpname", os_tmpname),
    ] {
        let fv = vm.lib_native(f)?;
        set_field(vm, os, name, fv)?;
    }
    // the locale in effect per category, which `os.setlocale` reads and
    // sets: strings, so a script reading them through `debug.getupvalue`
    // cannot change them
    let c = vm.str_value(b"C")?;
    let sl = vm.lib_native_with(os_setlocale, &[c; LC_COUNT])?;
    set_field(vm, os, "setlocale", sl)?;
    vm.lib_set_global("os", Value::Table(os))?;
    vm.barrier_back_table(os);
    Ok(())
}

/// [`open_file_loaders`], failing when memory runs out.
pub(crate) fn try_open_file_loaders(vm: &mut Vm) -> Result<(), LuaError> {
    let f = vm.lib_native(nat_loadfile)?;
    vm.lib_set_global("loadfile", f)?;
    let f = vm.lib_native(nat_dofile)?;
    vm.lib_set_global("dofile", f)?;
    Ok(())
}

fn set_field(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) -> Result<(), LuaError> {
    let k = vm.str_value(k.as_bytes())?;
    vm.lib_set_value(t, k, v)?;
    Ok(())
}
