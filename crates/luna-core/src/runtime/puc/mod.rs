//! The PUC model: PUC's `totalbytes` and what changes it, PUC's sizes of
//! its structures, its collector's pace and steps, and the records of
//! objects PUC has and luna does not. luna's own collector (`heap`) reaches
//! it through `for_heap`, the counting entries every allocation goes
//! through (`count.rs`, `parts.rs`, `Heap::counting`, `Heap::uncounted`,
//! `Heap::puc_size`) and the methods of the records it keeps (`records`);
//! the VM, the libraries and the JIT through the traits of `api`.

use std::ptr;

use crate::runtime::function::{LuaClosure, NativeClosure, Proto};
use crate::runtime::heap::*;
use crate::runtime::string::{self, LuaStr};
use crate::runtime::table::Table;
use crate::runtime::userdata::Userdata;
use crate::runtime::value::Value;
use records::{parse, strcache};

pub(crate) mod api;
pub(crate) mod boot;
mod control;
mod count;
pub(crate) use count::*;
mod defer;
pub(in crate::runtime) mod for_heap;
mod lent;
pub(crate) use lent::Lent;
mod load_session;
mod loads;
pub(crate) use defer::Deferred;
pub(crate) mod fmt;
mod obj;
pub(crate) mod pace;
mod params;
pub(crate) use obj::PucObj;
pub(crate) use params::{GcParams, LUNA_PAUSE, LUNA_STEPMUL, Param};
mod parts;
mod proto;
pub(crate) use proto::ENV51;
pub(crate) mod records;
pub(crate) mod size;
mod state;
pub(crate) mod step;
#[cfg(feature = "gc-trace")]
pub(crate) mod trace;
#[cfg(feature = "gc-verify")]
pub(crate) mod verify;
pub(crate) use state::{GC_LIMIT_OFFSET, PucModel};
mod thread;
pub(crate) use thread::{BASIC_CI_51, Thread};
#[cfg(test)]
mod tests;
pub(crate) mod work;
pub(crate) use work::native_puc_upvals;
