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
    K(&'static [u8]),
}

/// What `d`'s `luaL_openlibs` does to its library table `name` (as
/// `libshape.lua` names them), from the table's making, in order.
pub(crate) fn lib_layout(d: Dialect, name: &str) -> impl Iterator<Item = LibOp> + use<> {
    let mut rec = match d {
        Dialect::L51 => layout::layout_51(name),
        Dialect::L52 => layout::layout_52(name),
        Dialect::L53 => layout::layout_53(name),
        Dialect::L54 => layout::layout_54(name),
        Dialect::L55 => layout::layout_55(name),
    };
    std::iter::from_fn(move || {
        let (&tag, rest) = rec.split_first()?;
        if tag == b'R' {
            let size = |b: &[u8]| u32::from(u16::from_le_bytes([b[0], b[1]]));
            let op = LibOp::R(size(&rest[..2]), size(&rest[2..4]));
            rec = &rest[4..];
            Some(op)
        } else {
            let n = usize::from(rest[0]);
            let op = LibOp::K(&rest[1..1 + n]);
            rec = &rest[1 + n..];
            Some(op)
        }
    })
}
