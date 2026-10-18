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
pub(crate) use load::{nat_dofile, nat_loadfile};
use system::*;
use time::*;
mod puc_fns;
pub(crate) use puc_fns::PUC_FNS;

/// [`open_file_loaders`], failing when memory runs out.
pub(crate) fn try_open_file_loaders(vm: &mut Vm) -> Result<(), LuaError> {
    let f = vm.lib_native(nat_loadfile)?;
    vm.lib_set_global("loadfile", f)?;
    let f = vm.lib_native(nat_dofile)?;
    vm.lib_set_global("dofile", f)?;
    Ok(())
}
