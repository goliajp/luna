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

#[allow(clippy::all)]
#[rustfmt::skip]
#[path = "puc_lib_layout.rs"]
mod layout;

/// What PUC's `luaL_openlibs` does to a library table's parts.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LibOp {
    /// `luaH_resize` to an array part and a hash part of these sizes
    R(u32, u32),
    /// the string key enters the hash part
    K(&'static str),
}

/// What `d`'s `luaL_openlibs` does to each library table it names (as
/// `libshape.lua` names them), from the table's making.
pub(crate) fn lib_layouts(d: Dialect) -> &'static [(&'static str, &'static [LibOp])] {
    match d {
        Dialect::L51 => layout::LAYOUT_51,
        Dialect::L52 => layout::LAYOUT_52,
        Dialect::L53 => layout::LAYOUT_53,
        Dialect::L54 => layout::LAYOUT_54,
        Dialect::L55 => layout::LAYOUT_55,
    }
}
