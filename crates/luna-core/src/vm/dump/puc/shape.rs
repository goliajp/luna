//! What PUC's undumper allocates loading a binary chunk, in its order:
//! read again from the chunk's bytes, which the translator has accepted.

/// One allocation step of `luaU_undump` (with what `f_parser` does around
/// it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UEv<'a> {
    /// a Lua closure of this many upvalues (`luaF_newLclosure`)
    Closure(u8),
    /// 5.2: the closure made with one upvalue is replaced by one of this
    /// many
    Reclose(u8),
    /// a prototype (`luaF_newproto`), its function read up to [`UEv::End`]
    Proto,
    /// the function read since the matching [`UEv::Proto`] ends
    End,
    /// an array of the innermost function (`luaM_newvector`)
    Vec(Arr, u32),
    /// a string (5.5: saved for reuse, at the next index of the list)
    Str(&'a [u8]),
    /// 5.1 and 5.2: the load buffer grown to hold a string (`luaZ_openspace`)
    Buff(u32),
}

mod stream;
pub(crate) use stream::Undumping;

/// The arrays of a prototype, in [`crate::runtime::Proto::puc_arrays`]
/// order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Arr {
    Code,
    Protos,
    Consts,
    Lines,
    AbsLines,
    LocVars,
    Upvals,
}

/// The steps of loading `bytes`, a binary chunk of dialect `version` (the
/// version byte); `None` if it does not read as one to its end.
pub(crate) fn undump_events(bytes: &[u8], _header: usize) -> Option<Vec<UEv<'_>>> {
    let mut u = Undumping::new(true);
    u.feed(bytes);
    u.events(bytes)
}
