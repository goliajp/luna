//! The PUC model: PUC's `totalbytes` and what changes it, PUC's sizes of
//! its structures, its collector's pace and steps, and the records of
//! objects PUC has and luna does not. luna's own collector (`heap`) reaches
//! it through `for_heap`, the counting entries every allocation goes
//! through (`count`, `parts.rs`, `Heap::counting`, `Heap::uncounted`,
//! `Heap::puc_size`) and the methods of the records it keeps (`records`);
//! the VM, the libraries and the JIT through the facade `api` describes:
//! the model's `pub(crate)` heap methods and the types `api` names, the
//! only module visible outside the runtime.

use std::ptr;

use crate::runtime::function::{LuaClosure, NativeClosure, Proto};
use crate::runtime::heap::*;
use crate::runtime::string::{self, LuaStr};
use crate::runtime::table::Table;
use crate::runtime::userdata::Userdata;
use crate::runtime::value::Value;

pub(crate) mod api;
mod boot;
mod count;
pub(in crate::runtime) use count::{FIXED_EXT, PucKind};
pub(in crate::runtime) mod for_heap;
mod lent;
pub(in crate::runtime) use lent::Lent;
pub(in crate::runtime) mod fmt;
mod obj;
pub(in crate::runtime) mod pace;
mod params;
pub(in crate::runtime) use obj::PucObj;
pub(in crate::runtime) use params::{GcParams, Param};
mod parts;
mod proto;
pub(in crate::runtime) use proto::ENV51;
pub(in crate::runtime) mod records;
pub(in crate::runtime) mod size;
mod state;
pub(in crate::runtime) mod step;
#[cfg(feature = "gc-trace")]
pub(in crate::runtime) mod trace;
#[cfg(feature = "gc-verify")]
pub(in crate::runtime) mod verify;
pub(in crate::runtime) use state::PucModel;
mod thread;
pub(in crate::runtime) use thread::{BASIC_CI_51, Thread};
#[cfg(test)]
mod tests;
pub(in crate::runtime) mod work;
pub(in crate::runtime) use work::native_puc_upvals;
