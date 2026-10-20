//! 5.3's repeated number constants. Its code generator keys the constants
//! of every function of a chunk in one table, so a function adds a number
//! again when another function took its key in between; the code that
//! follows loads the new copy. An immediate operand names no constant, and
//! a dump gives it the first one of its value: the compiler keeps the
//! immediate form only where that is the constant PUC's code names.

use super::const_map::{ConstKey, DumpConstMap};
use super::*;

/// A number by its type and bits.
pub(crate) fn num_key(c: &Value) -> Option<ConstKey> {
    match *c {
        Value::Int(i) => Some(ConstKey::Int(i)),
        Value::Float(f) => Some(ConstKey::Float(f.to_bits())),
        _ => None,
    }
}

/// Each number's first index in `consts`, by [`num_key`].
pub(crate) fn first_nums(consts: &[Value]) -> DumpConstMap {
    let mut map = DumpConstMap::default();
    for (i, c) in consts.iter().enumerate() {
        if let Some(k) = num_key(c) {
            map.entry(k).or_insert(i as u32);
        }
    }
    map
}

/// Constant `i`, `c`, was just added to `l`'s function.
pub(super) fn note(l: &mut Level<'_>, c: &Value, i: u32) {
    if let Some(k) = num_key(c)
        && l.num_first.get(&k).is_none()
    {
        l.num_first.insert_or_abort(k, i);
    }
}

impl Compiler<'_> {
    /// Whether 5.3's constant `i`, the number `c`, is the first of its
    /// value in the running function.
    pub(super) fn first_num_const(&mut self, c: &Value, i: u32) -> bool {
        num_key(c).is_none_or(|k| self.l().num_first.get(&k) == Some(i))
    }
}
