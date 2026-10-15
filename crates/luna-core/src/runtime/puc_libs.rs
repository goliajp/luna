//! The record of what PUC's `luaL_openlibs` allocates
//! (`puc_libs_data.rs`), by dialect and pointer width.

use crate::runtime::table::Dialect;

#[allow(clippy::all)]
#[rustfmt::skip]
#[path = "puc_libs_data.rs"]
mod data;

/// The ops and strings of `d`'s record on this pointer width; `stopped`,
/// 5.3's with no collection running while the libraries open (traces of
/// runs with no automatic step).
pub(crate) fn record(d: Dialect, stopped: bool) -> (&'static [u32], &'static [&'static [u8]]) {
    use data::*;
    if cfg!(target_pointer_width = "64") {
        match d {
            Dialect::L51 => (OPS_51_64, STRS_51_64),
            Dialect::L52 => (OPS_52_64, STRS_52_64),
            Dialect::L53 if stopped => (OPS_53N_64, STRS_53N_64),
            Dialect::L53 => (OPS_53_64, STRS_53_64),
            Dialect::L54 => (OPS_54_64, STRS_54_64),
            Dialect::L55 => (OPS_55_64, STRS_55_64),
        }
    } else {
        match d {
            Dialect::L51 => (OPS_51_32, STRS_51_32),
            Dialect::L52 => (OPS_52_32, STRS_52_32),
            Dialect::L53 if stopped => (OPS_53N_32, STRS_53N_32),
            Dialect::L53 => (OPS_53_32, STRS_53_32),
            Dialect::L54 => (OPS_54_32, STRS_54_32),
            Dialect::L55 => (OPS_55_32, STRS_55_32),
        }
    }
}
