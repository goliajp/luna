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
pub(crate) use load::nat_loadfile;
use load::*;
use system::*;
use time::*;

pub(crate) fn open_os_io(vm: &mut Vm) {
    open_os(vm);
    lib_io::open_io(vm);
    open_file_loaders(vm);
}

/// The os library alone.
pub(crate) fn open_os(vm: &mut Vm) {
    let os = vm.heap.new_table();
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
        let fv = vm.native(f);
        set_field(vm, os, name, fv);
    }
    // the locale in effect per category, which `os.setlocale` reads and
    // sets: strings, so a script reading them through `debug.getupvalue`
    // cannot change them
    let c = Value::Str(vm.heap.intern(b"C"));
    let sl = vm.native_with(os_setlocale, vec![c; LC_COUNT].into_boxed_slice());
    set_field(vm, os, "setlocale", sl);
    vm.set_global("os", Value::Table(os))
        .expect("stdlib registration");
    vm.barrier_back_table(os);
}

/// The base functions that read files, `loadfile` and `dofile`.
pub(crate) fn open_file_loaders(vm: &mut Vm) {
    let f = vm.native(nat_loadfile);
    vm.set_global("loadfile", f).expect("stdlib registration");
    let f = vm.native(nat_dofile);
    vm.set_global("dofile", f).expect("stdlib registration");
}

fn set_field(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let k = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: `t` is a table the caller allocated or holds in a local, so it is alive; no reference into it is held across this call, and `set` does not collect
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, v)
        .expect("valid key");
}
