//! Records of objects PUC has and luna does not: what they count, how the
//! collector marks and sweeps them.

use super::*;

pub(in crate::runtime) mod env51;
pub(in crate::runtime::puc) mod ghost;
pub(crate) use ghost::{Ghost, GhostRef};
pub(in crate::runtime) mod open_upvals;
pub(in crate::runtime) mod parse;
pub(in crate::runtime) mod strcache;
pub(in crate::runtime) mod strt;
