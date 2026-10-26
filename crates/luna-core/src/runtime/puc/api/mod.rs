//! The model as the VM sees it. Code outside the runtime reaches the PUC
//! model only through these traits, which the heap implements; every other
//! entry of `puc` is visible to the runtime alone, so the compiler keeps
//! the VM to what is listed here.

mod collector;
mod count;
mod loads;
mod parse;
mod text;

pub(crate) use collector::PucCollector;
pub(crate) use count::PucCount;
pub(crate) use loads::PucLoads;
pub(crate) use parse::PucParse;
pub(crate) use text::PucText;
