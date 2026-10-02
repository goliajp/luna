//! The constant map of a function being compiled.

use super::Compiler;
use crate::runtime::Value;
use crate::runtime::string::LuaStr;
use std::collections::HashMap;

/// The hasher of a function's constant map: a multiply-rotate over the
/// key's words. The keys are a handful of numbers and string pointers per
/// function, where SipHash's setup cost dominates the lookup.
#[derive(Default)]
pub(super) struct ConstHasher(u64);

impl std::hash::Hasher for ConstHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn write_isize(&mut self, v: isize) {
        self.write_u64(v as u64);
    }
}

pub(super) type ConstMap = HashMap<ConstKey, u32, std::hash::BuildHasherDefault<ConstHasher>>;

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
        if let Some(&i) = l.const_map.get(&key) {
            return i;
        }
        let i = l.consts.len() as u32;
        l.consts.push(v);
        l.const_map.insert(key, i);
        i
    }
}
