//! Records of objects PUC has and luna does not: what they count, how the
//! collector marks and sweeps them.

use super::*;

pub(crate) mod env51;
mod ghost;
pub(crate) use ghost::{Ghost, GhostRef};
pub(crate) mod keeper;
pub(crate) mod open_upvals;
pub(crate) mod parse;
pub(crate) mod strcache;
pub(crate) mod strt;
