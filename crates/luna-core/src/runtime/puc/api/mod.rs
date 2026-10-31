//! The model as the code outside the runtime sees it. Its facade is every
//! `pub(crate)` method of the heap the model defines, which live in:
//! - this module's files: the collector and its pace (`collector.rs`), 5.1's
//!   environments (`env51.rs`), the records of objects luna has none of
//!   (`ghost.rs`), a load's span (`load_session.rs`), open upvalues
//!   (`open_upvals.rs`), the standard libraries' opening (`opening.rs`),
//!   the parser's replay (`parse.rs`), the strings PUC makes for text
//!   (`text.rs`)
//! - `count`: PUC's count and what moves it (`total.rs`, `objects.rs`,
//!   `blocks.rs`, `defer.rs`) and the loads under way (`loads.rs`), whose
//!   fields are that module's alone
//!
//! Besides those methods, the facade is the collector's steps the VM
//! drives (`step.rs`, run on the VM as their `StepHost`), the marker's
//! `keep` (`mark.rs`), and the types their arguments and results name,
//! re-exported below with their own methods. The model's other items are
//! visible to the runtime alone, so the compiler refuses any other way in.

mod collector;
mod env51;
mod ghost;
mod load_session;
mod mark;
mod open_upvals;
mod opening;
mod parse;
mod step;
mod text;

pub(crate) use super::count::{CountingMark, CountingOff, Deferred, GC_LIMIT_OFFSET};
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
pub(crate) use super::step::{CiCount, StepHost};
pub(crate) use super::thread::Thread;
