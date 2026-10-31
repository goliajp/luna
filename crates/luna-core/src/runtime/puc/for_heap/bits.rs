//! The model's bits in an object's header that the heap's marking, its
//! barrier, its string table and its free path touch.

use super::*;
use crate::runtime::puc::records::parse::{KEPT, kept_bit};
use crate::runtime::puc::records::strt::SWEPT;

/// The bits the model sets in a string's `sub` that keep it off the
/// plain free path: fixed by an external string, or swept off PUC's
/// string table.
pub(in crate::runtime) const STR_MODEL_BITS: u8 = FIXED_EXT | SWEPT;

/// The string `h` is marked or found again: PUC holds it, not only a load
/// in progress.
#[inline(always)]
pub(in crate::runtime) fn string_held(h: &mut GcHeader) {
    h.cls &= !KEPT;
}

/// The object `h` is marked: a string as [`string_held`]; another object's
/// bit there means something else and stays.
#[inline(always)]
pub(in crate::runtime) fn object_marked(h: &mut GcHeader) {
    h.cls &= !kept_bit(h.tag);
}

/// Whether the string sweep took the string `h` off PUC's table, which
/// PUC makes again now that it is found: cleared.
#[inline(always)]
pub(in crate::runtime) fn take_swept(h: &mut GcHeader) -> bool {
    let swept = h.sub & SWEPT != 0;
    h.sub &= !SWEPT;
    swept
}
