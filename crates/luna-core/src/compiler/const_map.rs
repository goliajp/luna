//! The constant map of a function being compiled.

use super::Compiler;
use crate::runtime::Value;
use crate::runtime::mem::{LMap, Oom};
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
    pub(super) fn const_idx(&mut self, key: ConstKey, v: Value) -> Result<u32, Oom> {
        let l = self.l();
        if let Some(i) = l.const_map.get(&key) {
            return Ok(i);
        }
        let i = l.consts.len() as u32;
        // the map entry first: a failed push leaves no entry for a missing
        // constant
        l.consts.reserve(1)?;
        l.const_map.insert(key, i)?;
        l.consts.push(v)?;
        Ok(i)
    }
}
