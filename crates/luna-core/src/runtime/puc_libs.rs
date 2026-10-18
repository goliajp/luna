//! The record of what PUC's `luaL_openlibs` allocates
//! (`puc_libs_data.rs`), by dialect and pointer width.

use crate::runtime::table::Dialect;

#[allow(clippy::all)]
#[rustfmt::skip]
#[path = "puc_libs_data.rs"]
mod data;

/// How `luaL_openlibs` runs: which of the records says what it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Opening {
    /// the collector stopped meanwhile
    Stopped,
    /// lua.c's: 5.3's leaves the collector running, the others stop it
    LuaC,
    /// an embedder's, after `luaL_newstate`, the collector running
    Running,
}

/// The ops and strings of `d`'s record of an `opening` on this pointer
/// width.
pub(crate) fn record(d: Dialect, opening: Opening) -> (&'static [u32], &'static [&'static [u8]]) {
    use Opening::*;
    use data::*;
    let o = if opening == LuaC && d != Dialect::L53 {
        Stopped
    } else {
        opening
    };
    if cfg!(target_pointer_width = "64") {
        match (d, o) {
            (Dialect::L51, Running) => (OPS_51H_64, STRS_51H_64),
            (Dialect::L51, _) => (OPS_51_64, STRS_51_64),
            (Dialect::L52, Running) => (OPS_52H_64, STRS_52H_64),
            (Dialect::L52, _) => (OPS_52_64, STRS_52_64),
            (Dialect::L53, Running) => (OPS_53H_64, STRS_53H_64),
            (Dialect::L53, Stopped) => (OPS_53N_64, STRS_53N_64),
            (Dialect::L53, LuaC) => (OPS_53_64, STRS_53_64),
            (Dialect::L54, Running) => (OPS_54H_64, STRS_54H_64),
            (Dialect::L54, _) => (OPS_54_64, STRS_54_64),
            (Dialect::L55, Running) => (OPS_55H_64, STRS_55H_64),
            (Dialect::L55, _) => (OPS_55_64, STRS_55_64),
        }
    } else {
        match (d, o) {
            (Dialect::L51, Running) => (OPS_51H_32, STRS_51H_32),
            (Dialect::L51, _) => (OPS_51_32, STRS_51_32),
            (Dialect::L52, Running) => (OPS_52H_32, STRS_52H_32),
            (Dialect::L52, _) => (OPS_52_32, STRS_52_32),
            (Dialect::L53, Running) => (OPS_53H_32, STRS_53H_32),
            (Dialect::L53, Stopped) => (OPS_53N_32, STRS_53N_32),
            (Dialect::L53, LuaC) => (OPS_53_32, STRS_53_32),
            (Dialect::L54, Running) => (OPS_54H_32, STRS_54H_32),
            (Dialect::L54, _) => (OPS_54_32, STRS_54_32),
            (Dialect::L55, Running) => (OPS_55H_32, STRS_55H_32),
            (Dialect::L55, _) => (OPS_55_32, STRS_55_32),
        }
    }
}

/// Where an embedder's opening leaves `d`'s collector when that is in the
/// middle of a cycle, on this pointer width: `S state debt estimate
/// traversed`, then `B path` for each black object, `G path` for each gray
/// one in the order the collector takes them, `A path` for each one gray
/// again (a path from the registry `R` or the main thread `L`; a string by
/// its bytes, `S:` and hex).
pub(crate) fn midcycle(d: Dialect) -> Option<&'static [&'static str]> {
    use data::*;
    match d {
        Dialect::L52 if cfg!(target_pointer_width = "64") => Some(STATE_52H_64),
        Dialect::L52 => Some(STATE_52H_32),
        _ => None,
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
    /// the string key enters the hash part (read to check the libraries'
    /// order, in debug builds)
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
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
