//! The constant map of a function being compiled. Which constants share
//! an entry follows each dialect's PUC code generator (`addk`,
//! `luaK_numberK`, `k2proto`), so a dumped constant table is PUC's: the
//! map's key is the key PUC's scanner table uses, and an entry found under
//! it is reused only when PUC would reuse it.

use super::Compiler;
use crate::runtime::Value;
use crate::runtime::mem::{LMap, LVec};
use crate::runtime::string::{LuaStr, MAX_SHORT_LEN};
use crate::version::LuaVersion;
use std::collections::HashMap;

/// A function's constant map while it is compiled (allocated from the Vm's
/// allocation context).
pub(super) type ConstMap = LMap<ConstKey, u32>;

/// The same map for the dump writer, which has no allocation context.
pub(crate) type DumpConstMap = HashMap<ConstKey, u32>;

/// The scanner table the rules below read and write: either map.
pub(crate) trait KeyMap {
    fn find(&self, k: &ConstKey) -> Option<u32>;
    fn put(&mut self, k: ConstKey, i: u32);
}

impl KeyMap for ConstMap {
    fn find(&self, k: &ConstKey) -> Option<u32> {
        self.get(k)
    }
    fn put(&mut self, k: ConstKey, i: u32) {
        self.insert_or_abort(k, i);
    }
}

impl KeyMap for DumpConstMap {
    fn find(&self, k: &ConstKey) -> Option<u32> {
        self.get(k).copied()
    }
    fn put(&mut self, k: ConstKey, i: u32) {
        self.insert(k, i);
    }
}

/// The constant table the rules add to: either vector.
pub(crate) trait ConstList {
    fn items(&self) -> &[Value];
    fn add(&mut self, v: Value);
}

impl ConstList for LVec<Value> {
    fn items(&self) -> &[Value] {
        self
    }
    fn add(&mut self, v: Value) {
        self.push_or_abort(v);
    }
}

impl ConstList for Vec<Value> {
    fn items(&self) -> &[Value] {
        self
    }
    fn add(&mut self, v: Value) {
        self.push(v);
    }
}

/// A key of PUC's scanner table.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConstKey {
    /// an integer key (a float key with an integer value becomes one)
    Int(i64),
    /// a float key, by its bits
    Float(u64),
    /// 5.3's light-userdata key of an integer constant
    LightInt(i64),
    /// a short string (interned, so one object per content)
    Str(*mut LuaStr),
    /// a long string, or 5.2's eight-byte key of a zero or nan, by a hash
    /// and the length of its bytes (an eight-byte string keys this way too:
    /// PUC 5.2 notes it can collide with such a number)
    Bytes(u64, u32),
    /// 5.5's key for every zero
    Zero,
    /// a boolean keys by itself
    Bool(bool),
    /// nil, keyed by PUC by the scanner table itself: one entry
    Nil,
}

/// Where a constant goes: under a key (reused only when the entry there
/// passes `reuse`), or always into a new entry that no key names.
enum Plan {
    Key(ConstKey),
    New,
}

/// 2^-52 (PUC's `ldexp(1.0, -nbm + 1)` with 53-bit doubles).
const Q: f64 = 1.0 / 4_503_599_627_370_496.0;

/// The integer a float equals exactly, as `luaV_flttointeger(.., F2Ieq)`.
fn exact_int(f: f64) -> Option<i64> {
    // [-2^63, 2^63)
    (f == f.floor() && (i64::MIN as f64..-(i64::MIN as f64)).contains(&f)).then_some(f as i64)
}

/// A float as a key of a 5.3+ table: an integer value is an integer key.
fn float_key(f: f64) -> ConstKey {
    match exact_int(f) {
        Some(i) => ConstKey::Int(i),
        None => ConstKey::Float(f.to_bits()),
    }
}

fn bytes_key(b: &[u8]) -> ConstKey {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h = (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
    }
    ConstKey::Bytes(h, b.len() as u32)
}

/// PUC `luaV_rawequalobj` on two constants of dialect `v`: before 5.3
/// numbers have one type; NaN equals nothing.
fn raw_equal(v: LuaVersion, a: &Value, b: &Value) -> bool {
    let num = |x: &Value| match *x {
        Value::Int(i) => Some(i as f64),
        Value::Float(f) => Some(f),
        _ => None,
    };
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x.as_bytes() == y.as_bytes(),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        _ if v <= LuaVersion::Lua52 => matches!((num(a), num(b)), (Some(x), Some(y)) if x == y),
        _ => false,
    }
}

/// Where dialect `v` puts constant `c`, and whether an entry found under
/// the key is reused without comparing it (PUC asserts instead).
fn plan(v: LuaVersion, c: &Value) -> (Plan, bool) {
    let key = |k| (Plan::Key(k), false);
    match (v, c) {
        (_, Value::Str(s)) if s.len() > MAX_SHORT_LEN || s.len() == 8 => {
            key(bytes_key(s.as_bytes()))
        }
        (_, Value::Str(s)) => key(ConstKey::Str(s.as_ptr())),
        // 5.1: a number keys by its value; -0 and 0 are one key
        (LuaVersion::Lua51, Value::Int(i)) => (
            Plan::Key(ConstKey::Float((*i as f64 + 0.0).to_bits())),
            true,
        ),
        (LuaVersion::Lua51, Value::Float(f)) => {
            (Plan::Key(ConstKey::Float((f + 0.0).to_bits())), true)
        }
        // 5.2: zero and nan key by their bytes, as a string would
        (LuaVersion::Lua52, Value::Float(f)) if *f == 0.0 || f.is_nan() => {
            key(bytes_key(&f.to_le_bytes()))
        }
        (LuaVersion::Lua52, Value::Int(0)) => key(bytes_key(&0.0f64.to_le_bytes())),
        (LuaVersion::Lua52, Value::Int(i)) => key(ConstKey::Float((*i as f64).to_bits())),
        (LuaVersion::Lua52, Value::Float(f)) => key(ConstKey::Float(f.to_bits())),
        (LuaVersion::Lua53, Value::Int(i)) => key(ConstKey::LightInt(*i)),
        (LuaVersion::Lua53, Value::Float(f)) => key(float_key(*f)),
        // 5.4: an integral float keys by itself nudged off the integers
        (LuaVersion::Lua54, Value::Float(f)) => match exact_int(*f) {
            Some(0) => key(float_key(Q)),
            Some(_) => key(float_key(f + f * Q)),
            None => key(ConstKey::Float(f.to_bits())),
        },
        // 5.5: every zero shares one key; a nudged key that is still an
        // integer is not cached
        (LuaVersion::Lua55, Value::Float(f)) if *f == 0.0 => (Plan::Key(ConstKey::Zero), true),
        (LuaVersion::Lua55, Value::Float(f)) => {
            let k = f * (1.0 + Q);
            match exact_int(k) {
                Some(_) => (Plan::New, false),
                None => key(ConstKey::Float(k.to_bits())),
            }
        }
        (LuaVersion::Lua55, Value::Int(i)) => (Plan::Key(ConstKey::Int(*i)), true),
        (_, Value::Int(i)) => key(ConstKey::Int(*i)),
        (_, Value::Float(f)) => key(float_key(*f)),
        (_, Value::Bool(b)) => (Plan::Key(ConstKey::Bool(*b)), true),
        (_, Value::Nil) => (Plan::Key(ConstKey::Nil), true),
        _ => (Plan::New, false),
    }
}

/// The index of constant `c` in `consts` of a function of dialect `v`,
/// whose scanner table is `map`: added when PUC would add a new entry.
pub(crate) fn add_const(
    v: LuaVersion,
    consts: &mut impl ConstList,
    map: &mut impl KeyMap,
    c: Value,
) -> u32 {
    let (plan, trusted) = plan(v, &c);
    let key = match plan {
        Plan::Key(key) => {
            if let Some(i) = map.find(&key)
                && (trusted || raw_equal(v, &consts.items()[i as usize], &c))
            {
                return i;
            }
            Some(key)
        }
        Plan::New => None,
    };
    let i = consts.items().len() as u32;
    consts.add(c);
    if let Some(key) = key {
        remember(v, map, key, i);
    }
    i
}

/// Records a new entry `i` under `key`; 5.5 caches a float only under a
/// key that was free.
fn remember(v: LuaVersion, map: &mut impl KeyMap, key: ConstKey, i: u32) {
    if v == LuaVersion::Lua55 && matches!(key, ConstKey::Float(_)) && map.find(&key).is_some() {
        return;
    }
    map.put(key, i);
}

/// The scanner table PUC would have after adding `consts` in order, each
/// as a new entry (to go on adding constants to a finished function).
pub(crate) fn const_map_of(v: LuaVersion, consts: &[Value]) -> DumpConstMap {
    let mut map = DumpConstMap::default();
    for (i, c) in consts.iter().enumerate() {
        if let (Plan::Key(key), _) = plan(v, c) {
            remember(v, &mut map, key, i as u32);
        }
    }
    map
}

impl Compiler<'_> {
    /// The index of constant `c` in the running function (see
    /// [`add_const`]).
    pub(super) fn const_idx(&mut self, c: Value) -> u32 {
        let v = self.version;
        let l = self.l();
        add_const(v, &mut l.consts, &mut l.const_map, c)
    }
}
