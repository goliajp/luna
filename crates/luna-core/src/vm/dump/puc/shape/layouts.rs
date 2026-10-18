//! The layout of a function in each binary chunk format, as steps of
//! PUC's undumper.

pub(super) use super::super::Arr;

/// What a count or a string's size is read as.
#[derive(Clone, Copy)]
pub(super) enum Num {
    U32,
    V54,
    V55,
}

/// The string formats: 5.1 and 5.2, 5.3, 5.4, 5.5.
#[derive(Clone, Copy)]
pub(super) enum Sv {
    S51,
    S53,
    S54,
    S55,
}

/// One step of a function's layout.
#[derive(Clone, Copy)]
pub(super) enum St {
    /// a nested function's prototype made (5.1 makes its own)
    Proto,
    /// the function read to its end
    End,
    /// a string; `true`: the function's source
    Str(Sv, bool),
    Take(usize),
    /// line defined, last line, parameters, vararg and stack size, the
    /// lines in 4 bytes (`Some`) or variable-length
    Head(Option<()>, Num),
    /// a count, kept for the steps after it, with the array it sizes
    Count(Num, Option<Arr>),
    /// the array the count sizes, made
    VecOf(Arr),
    /// the count times this many bytes
    TakeN(usize),
    Var(Num),
    /// 5.5 `loadAlign(4)`
    Align4,
    /// each of the count, this
    Each(&'static [St]),
    /// a constant
    Const(Sv),
    /// the count is the function's upvalues
    Ups,
    /// 5.5's absolute line info and upvalue names
    AbsLines,
    Names,
    /// each of the count, a nested function
    Funcs,
    Fn,
}

pub(super) use super::super::Arr as A;
pub(super) use Num::{U32, V54, V55};
pub(super) use St::*;

pub(super) const LOCVAR51: &[St] = &[Str(Sv::S51, false), Take(8)];
pub(super) const NAME51: &[St] = &[Str(Sv::S51, false)];
pub(super) const F51: &[St] = &[
    Proto,
    Str(Sv::S51, false),
    Take(12),
    Count(U32, Some(A::Code)),
    TakeN(4),
    Count(U32, Some(A::Consts)),
    Each(&[Const(Sv::S51)]),
    Count(U32, Some(A::Protos)),
    Funcs,
    Count(U32, Some(A::Lines)),
    TakeN(4),
    Count(U32, Some(A::LocVars)),
    Each(LOCVAR51),
    Count(U32, Some(A::Upvals)),
    Each(NAME51),
    End,
];
pub(super) const F52: &[St] = &[
    Head(Some(()), U32),
    Count(U32, Some(A::Code)),
    TakeN(4),
    Count(U32, Some(A::Consts)),
    Each(&[Const(Sv::S51)]),
    Count(U32, Some(A::Protos)),
    Funcs,
    Count(U32, Some(A::Upvals)),
    Ups,
    TakeN(2),
    Str(Sv::S51, true),
    Count(U32, Some(A::Lines)),
    TakeN(4),
    Count(U32, Some(A::LocVars)),
    Each(LOCVAR51),
    Count(U32, None),
    Each(NAME51),
    End,
];
pub(super) const F53: &[St] = &[
    Str(Sv::S53, true),
    Head(Some(()), U32),
    Count(U32, Some(A::Code)),
    TakeN(4),
    Count(U32, Some(A::Consts)),
    Each(&[Const(Sv::S53)]),
    Count(U32, Some(A::Upvals)),
    TakeN(2),
    Count(U32, Some(A::Protos)),
    Funcs,
    Count(U32, Some(A::Lines)),
    TakeN(4),
    Count(U32, Some(A::LocVars)),
    Each(&[Str(Sv::S53, false), Take(8)]),
    Count(U32, None),
    Each(&[Str(Sv::S53, false)]),
    End,
];
pub(super) const F54: &[St] = &[
    Str(Sv::S54, true),
    Head(None, V54),
    Count(V54, Some(A::Code)),
    TakeN(4),
    Count(V54, Some(A::Consts)),
    Each(&[Const(Sv::S54)]),
    Count(V54, Some(A::Upvals)),
    TakeN(3),
    Count(V54, Some(A::Protos)),
    Funcs,
    Count(V54, Some(A::Lines)),
    TakeN(1),
    Count(V54, Some(A::AbsLines)),
    Each(&[Var(V54), Var(V54)]),
    Count(V54, Some(A::LocVars)),
    Each(&[Str(Sv::S54, false), Var(V54), Var(V54)]),
    Count(V54, None),
    Each(&[Str(Sv::S54, false)]),
    End,
];
pub(super) const F55: &[St] = &[
    Head(None, V55),
    Count(V55, None),
    Align4,
    VecOf(A::Code),
    TakeN(4),
    Count(V55, Some(A::Consts)),
    Each(&[Const(Sv::S55)]),
    Count(V55, Some(A::Upvals)),
    Ups,
    TakeN(3),
    Count(V55, Some(A::Protos)),
    Funcs,
    Str(Sv::S55, true),
    Count(V55, Some(A::Lines)),
    TakeN(1),
    AbsLines,
    Count(V55, Some(A::LocVars)),
    Each(&[Str(Sv::S55, false), Var(V55), Var(V55)]),
    Names,
    End,
];

/// Nesting deeper than this is refused, as the translator refuses it.
pub(super) const DEEP: u32 = 250;
