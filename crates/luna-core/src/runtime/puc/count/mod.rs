//! PUC's count and the loads it is kept around: `totalbytes` and what
//! moves it (objects, blocks, strings, a load's reader counted aside), and
//! the state of the loads under way. The fields of both are this module's
//! alone; the rest of the model and the VM read and move them through the
//! heap's methods defined here.

mod blocks;
mod defer;
mod loads;
mod objects;
mod total;

pub(crate) use defer::Deferred;
pub(in crate::runtime::puc) use loads::{Loads, LoadsAside, ParseMark};
pub(in crate::runtime) use objects::{FIXED_EXT, PucKind};
pub(in crate::runtime::puc) use total::Count;
pub(crate) use total::{CountingMark, CountingOff, GC_LIMIT_OFFSET};
