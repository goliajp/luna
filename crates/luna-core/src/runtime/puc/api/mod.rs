//! The model as the code outside the runtime sees it, and all of it that
//! code reaches: the heap's methods defined in these files, the collector's
//! steps the VM drives (`step.rs`, run on the VM as their `StepHost`), and
//! the types their arguments and results name, re-exported below. The
//! model's other modules are visible to the runtime alone, so the compiler
//! refuses any other way in.

mod collector;
mod count;
#[cfg(feature = "gc-trace")]
mod count_trace;
mod defer;
mod env51;
mod ghost;
mod load_session;
mod loads;
mod open_upvals;
mod opening;
mod parse;
mod step;
mod text;

pub(crate) use count::{CountingMark, CountingOff};
pub(crate) use defer::Deferred;
use load_session::unpaired;
pub(in crate::runtime::puc) use opening::Opening;
pub(crate) use step::{
    change_mode, full_collect, gc_finish_finalizers, gc_full_pause, gc_full_repause, gc_lua_step,
    gc_steps_to_pause, lua_step_skipped, puc_step,
};

// what the VM names of the model's types: the arguments and results of
// the methods above, and the records' kinds it makes
pub(crate) use super::fmt::FmtArg;
pub(crate) use super::obj::PucObj;
pub(crate) use super::params::{LUNA_PAUSE, LUNA_STEPMUL, Param};
pub(crate) use super::records::env51::{ENV_IO, ENV_IO_POPEN, ENV_IO_STD, ENV_PACKAGE, ENV_PROTO};
pub(crate) use super::records::open_upvals::OpenLend;
pub(crate) use super::records::parse::{
    FuncArrays, GrowVec, OpenView, PKey, PTab, ParseFrame, ParserArray, ParserSizes,
};
pub(crate) use super::records::strcache::{CStr, site};
pub(crate) use super::records::{Ghost, GhostRef};
pub(crate) use super::state::GC_LIMIT_OFFSET;
pub(crate) use super::step::{CiCount, StepHost};
pub(crate) use super::thread::Thread;
