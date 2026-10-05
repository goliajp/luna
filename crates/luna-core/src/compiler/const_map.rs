//! The constant map of a function being compiled.

use super::Compiler;
use crate::runtime::Value;
use crate::runtime::mem::LMap;
use crate::runtime::string::LuaStr;

pub(super) type ConstMap = LMap<ConstKey, u32>;

/// A constant as the map keys it: numbers by value (a float by its bits),
/// strings by object.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ConstKey {
    Int(i64),
    Float(u64),
    Str(*mut LuaStr),
}

impl Compiler<'_> {
    /// The index of constant `v` (keyed `key`) in the running function,
    /// added when new.
    pub(super) fn const_idx(&mut self, key: ConstKey, v: Value) -> u32 {
        let l = self.l();
        if let Some(i) = l.const_map.get(&key) {
            return i;
        }
        let i = l.consts.len() as u32;
        l.consts.push_or_abort(v);
        l.const_map.insert_or_abort(key, i);
        i
    }
}
