//! The PUC model: PUC's `totalbytes` and what changes it, PUC's sizes of
//! its structures, its collector's pace and steps, and the records of
//! objects PUC has and luna does not. luna's own collector (`heap`) calls
//! into it only through the entries listed in the refactor plan.

use std::ptr;

use crate::runtime::function::{LuaClosure, NativeClosure, Proto};
use crate::runtime::heap::*;
use crate::runtime::string::{self, LuaStr};
use crate::runtime::table::Table;
use crate::runtime::userdata::Userdata;
use crate::runtime::value::Value;

pub(crate) mod boot;
pub(crate) use boot::STRT_STAND;
mod count;
pub(crate) use count::*;
mod defer;
pub(crate) use defer::Deferred;
pub(crate) mod fmt;
pub(crate) mod pace;
mod parts;
mod proto;
pub(crate) use proto::ENV51;
pub(crate) mod records;
pub(crate) mod size;
mod sizer;
pub(crate) use sizer::Sizer;
mod state;
pub(crate) use state::PucModel;
mod thread;
pub(crate) use thread::{BASIC_CI_51, Thread};
#[cfg(test)]
mod tests;
pub(crate) mod work;
